//! Runs a form's JavaScript in a sandbox.
//!
//! A script never runs unless the person allowed it; this crate is only
//! called for scripts they approved. Even then the script sees very little:
//! a fresh engine for every run, holding nothing but the field values handed
//! in as plain data, an `event`, and a few Acrobat objects (written in
//! JavaScript, in `prelude.js`) that read and set those values. No function
//! reaches the file, the app, the network or the computer, and what the
//! script changes is handed back as data for the viewer to apply.
//!
//! Each run is bounded: loops, recursion and stack depth are capped. The
//! viewer runs scripts in a helper process of its own ([`run_isolated`]),
//! so a script that runs too long is killed after [`TIME_LIMIT`], and one
//! that exhausts memory or crashes takes only the helper down, never the
//! open document.

mod wire;

pub use wire::{decode_commit, decode_outcome, encode_commit, encode_outcome};

use std::sync::mpsc;
use std::time::Duration;

use boa_engine::object::builtins::JsArray;
use boa_engine::{Context, JsValue, Source, js_string};

/// How long one script may run.
pub const TIME_LIMIT: Duration = Duration::from_secs(2);
const LOOP_LIMIT: u64 = 2_000_000;
const RECURSION_LIMIT: usize = 400;
const STACK_LIMIT: usize = 16 * 1024;

const PRELUDE: &str = include_str!("prelude.js");

/// A field as the script sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldValue {
    /// The full name, such as `order.total`.
    pub name: String,
    /// The value as text ("Off" for a checkbox that is off).
    pub value: String,
    /// Acrobat's type name: text, checkbox, radiobutton, combobox, listbox,
    /// button or signature.
    pub kind: &'static str,
    /// Whether its widgets are hidden.
    pub hidden: bool,
}

/// What made the script run.
#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub enum Trigger {
    /// A value is being committed (`event.willCommit` is true).
    Keystroke,
    /// A committed value is checked.
    Validate,
    /// A field's value is worked out from others.
    Calculate,
    /// A value is turned into the text shown.
    Format,
}

impl Trigger {
    fn name(self) -> &'static str {
        match self {
            Self::Keystroke => "Keystroke",
            Self::Validate => "Validate",
            Self::Calculate => "Calculate",
            Self::Format => "Format",
        }
    }
}

/// One run: the document's own scripts (run first, to define functions),
/// the script, and what it runs on.
#[derive(Debug, Clone)]
pub struct Run {
    pub document_scripts: Vec<String>,
    pub script: String,
    pub trigger: Trigger,
    /// The field the event is for.
    pub target: String,
    /// `event.value` going in.
    pub value: String,
    pub fields: Vec<FieldValue>,
}

/// What the script did.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Outcome {
    /// `event.rc`: false when the script rejected the value.
    pub accepted: bool,
    /// `event.value` afterwards, as text.
    pub value: String,
    /// Other fields the script set, in name order.
    pub changes: Vec<(String, String)>,
    /// Fields the script showed (false) or hid (true), in name order.
    pub shown: Vec<(String, bool)>,
    /// Messages the script showed with `app.alert`.
    pub alerts: Vec<String>,
    /// Why the run stopped early, when it did.
    pub error: Option<String>,
}

/// A JavaScript string literal for `s`.
fn literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{2028}' || c == '\u{2029}' => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The data a run starts from, as JavaScript.
fn setup(run: &Run) -> String {
    let mut out = String::from("var __fields = Object.create(null);\n");
    for f in &run.fields {
        out.push_str(&format!(
            "__fields[{}] = {{ value: {}, kind: {}, hidden: {} }};\n",
            literal(&f.name),
            literal(&f.value),
            literal(f.kind),
            f.hidden
        ));
    }
    out
}

fn event_setup(run: &Run) -> String {
    format!(
        "var event = {{ name: {}, type: \"Field\", value: {}, change: \"\", \
         willCommit: true, rc: true, targetName: {}, target: getField({}), \
         modifier: false, shift: false, source: null }};\n\
         if (event.target === null) event.target = new Field({});\n",
        literal(run.trigger.name()),
        literal(&run.value),
        literal(&run.target),
        literal(&run.target),
        literal(&run.target),
    )
}

/// Runs a script in a fresh sandbox, on its own thread, giving up after
/// [`TIME_LIMIT`].
pub(crate) fn run(run: Run) -> Outcome {
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("form script".into())
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            let _ = tx.send(run_here(&run));
        });
    if let Err(e) = spawned {
        return failed(format!("could not start the script: {e}"));
    }
    match rx.recv_timeout(TIME_LIMIT) {
        Ok(outcome) => outcome,
        Err(mpsc::RecvTimeoutError::Timeout) => failed(format!(
            "the script was stopped after {} seconds",
            TIME_LIMIT.as_secs()
        )),
        Err(mpsc::RecvTimeoutError::Disconnected) => failed("the script engine failed".into()),
    }
}

fn failed(error: String) -> Outcome {
    Outcome {
        accepted: true,
        error: Some(error),
        ..Outcome::default()
    }
}

fn to_text(v: &JsValue, ctx: &mut Context) -> String {
    v.to_string(ctx)
        .map(|s| s.to_std_string_escaped())
        .unwrap_or_default()
}

/// Runs a script on this thread.
fn run_here(run: &Run) -> Outcome {
    let mut ctx = Context::default();
    {
        let limits = ctx.runtime_limits_mut();
        limits.set_loop_iteration_limit(LOOP_LIMIT);
        limits.set_recursion_limit(RECURSION_LIMIT);
        limits.set_stack_size_limit(STACK_LIMIT);
    }
    let mut out = Outcome {
        accepted: true,
        value: run.value.clone(),
        ..Outcome::default()
    };
    let eval = |ctx: &mut Context, code: &str, what: &str| -> Result<(), String> {
        ctx.eval(Source::from_bytes(code))
            .map(|_| ())
            .map_err(|e| format!("{what}: {e}"))
    };
    if let Err(e) = eval(&mut ctx, &setup(run), "setting up")
        .and_then(|()| eval(&mut ctx, PRELUDE, "setting up"))
    {
        out.error = Some(e);
        return out;
    }
    for (i, s) in run.document_scripts.iter().enumerate() {
        if let Err(e) = eval(&mut ctx, s, &format!("document script {}", i + 1)) {
            out.error = Some(e);
        }
    }
    let ran = eval(&mut ctx, &event_setup(run), "setting up")
        .and_then(|()| eval(&mut ctx, &run.script, "the script"));
    if let Err(e) = ran {
        out.error = Some(e);
    }
    // Read back what happened, whether or not the script finished.
    let global = ctx.global_object();
    if let Ok(event) = global.get(js_string!("event"), &mut ctx)
        && let Some(event) = event.as_object()
    {
        if let Ok(rc) = event.get(js_string!("rc"), &mut ctx) {
            out.accepted = rc.to_boolean();
        }
        if let Ok(v) = event.get(js_string!("value"), &mut ctx) {
            out.value = if v.is_null_or_undefined() {
                String::new()
            } else {
                to_text(&v, &mut ctx)
            };
        }
    }
    if let Ok(changed) = ctx.eval(Source::from_bytes(
        "Object.keys(__changed).sort().map(function (k) { return [k, __changed[k]]; })",
    )) && let Some(obj) = changed.as_object()
        && let Ok(list) = JsArray::from_object(obj.clone())
    {
        let n = list.length(&mut ctx).unwrap_or(0);
        for i in 0..n.min(10_000) {
            let Ok(pair) = list.get(i, &mut ctx) else {
                continue;
            };
            let Some(pair) = pair.as_object() else {
                continue;
            };
            let Ok(pair) = JsArray::from_object(pair.clone()) else {
                continue;
            };
            let name = pair.get(0, &mut ctx).map(|v| to_text(&v, &mut ctx));
            let value = pair.get(1, &mut ctx).map(|v| to_text(&v, &mut ctx));
            if let (Ok(name), Ok(value)) = (name, value) {
                out.changes.push((name, value));
            }
        }
    }
    if let Ok(shown) = ctx.eval(Source::from_bytes(
        "Object.keys(__shown).sort().map(function (k) { return [k, __shown[k]]; })",
    )) && let Some(obj) = shown.as_object()
        && let Ok(list) = JsArray::from_object(obj.clone())
    {
        let n = list.length(&mut ctx).unwrap_or(0);
        for i in 0..n.min(10_000) {
            let Ok(pair) = list.get(i, &mut ctx) else {
                continue;
            };
            let Some(pair) = pair.as_object() else {
                continue;
            };
            let Ok(pair) = JsArray::from_object(pair.clone()) else {
                continue;
            };
            let name = pair.get(0, &mut ctx).map(|v| to_text(&v, &mut ctx));
            let hidden = pair.get(1, &mut ctx).map(|v| v.to_boolean());
            if let (Ok(name), Ok(hidden)) = (name, hidden) {
                out.shown.push((name, hidden));
            }
        }
    }
    if let Ok(alerts) = global.get(js_string!("__alerts"), &mut ctx)
        && let Some(obj) = alerts.as_object()
        && let Ok(list) = JsArray::from_object(obj.clone())
    {
        let n = list.length(&mut ctx).unwrap_or(0);
        for i in 0..n.min(20) {
            if let Ok(v) = list.get(i, &mut ctx) {
                out.alerts.push(to_text(&v, &mut ctx));
            }
        }
    }
    out
}

/// A value being committed to a field, and the scripts that follow from it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Commit {
    pub document_scripts: Vec<String>,
    pub fields: Vec<FieldValue>,
    /// The field being changed, and its new value.
    pub target: String,
    pub value: String,
    /// The field's keystroke script (run with `willCommit` true) and its
    /// validate script.
    pub keystroke: Option<String>,
    pub validate: Option<String>,
    /// Calculate scripts, in the form's calculation order: the field each
    /// sets, and the script.
    pub calculations: Vec<(String, String)>,
    /// Validate scripts of the calculated fields, run on each calculated
    /// value before it is set.
    pub calculated_validate: Vec<(String, String)>,
}

/// What the scripts made of a commit.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CommitOutcome {
    /// False when a keystroke or validate script rejected the value.
    pub accepted: bool,
    /// The value to commit, as the scripts left it.
    pub value: String,
    /// Other fields' new values, from calculations and from scripts that
    /// set fields, in the order they were set.
    pub changes: Vec<(String, String)>,
    /// Fields shown (false) or hidden (true), by name; a name can be a
    /// parent of several fields.
    pub hidden: Vec<(String, bool)>,
    pub alerts: Vec<String>,
    pub errors: Vec<String>,
}

impl CommitOutcome {
    fn note(&mut self, what: &str, out: &Outcome, fields: &mut [FieldValue]) {
        self.alerts.extend(out.alerts.iter().cloned());
        for (name, hidden) in &out.shown {
            let prefix = format!("{name}.");
            for f in fields
                .iter_mut()
                .filter(|f| f.name == *name || f.name.starts_with(&prefix))
            {
                f.hidden = *hidden;
            }
            self.hidden.retain(|(n, _)| n != name);
            self.hidden.push((name.clone(), *hidden));
        }
        if let Some(e) = &out.error {
            self.errors.push(format!("{what}: {e}"));
        }
    }

    fn set(&mut self, fields: &mut [FieldValue], name: &str, value: &str) {
        if let Some(f) = fields.iter_mut().find(|f| f.name == name) {
            if f.value == value {
                return;
            }
            f.value = value.to_string();
        }
        self.changes.retain(|(n, _)| n != name);
        self.changes.push((name.to_string(), value.to_string()));
    }
}

/// Runs the scripts for a commit, here: keystroke and validate on the new
/// value, then every calculation in order, each seeing the values before
/// it. The helper process calls this; the viewer calls [`run_isolated`].
pub fn run_commit(c: &Commit) -> CommitOutcome {
    let mut fields = c.fields.clone();
    let mut out = CommitOutcome {
        accepted: true,
        value: c.value.clone(),
        ..CommitOutcome::default()
    };
    let run_one = |fields: &[FieldValue], script: &str, trigger, target: &str, value: &str| {
        run(Run {
            document_scripts: c.document_scripts.clone(),
            script: script.to_string(),
            trigger,
            target: target.to_string(),
            value: value.to_string(),
            fields: fields.to_vec(),
        })
    };
    for (script, trigger, what) in [
        (&c.keystroke, Trigger::Keystroke, "Keystroke"),
        (&c.validate, Trigger::Validate, "Validate"),
    ] {
        let Some(script) = script else { continue };
        let o = run_one(&fields, script, trigger, &c.target, &out.value);
        out.note(&format!("{what} script of {}", c.target), &o, &mut fields);
        if !o.accepted {
            out.accepted = false;
            return out;
        }
        out.value = o.value;
    }
    if let Some(f) = fields.iter_mut().find(|f| f.name == c.target) {
        f.value = out.value.clone();
    }
    for (target, script) in &c.calculations {
        let current = fields
            .iter()
            .find(|f| f.name == *target)
            .map(|f| f.value.clone())
            .unwrap_or_default();
        let o = run_one(&fields, script, Trigger::Calculate, target, &current);
        out.note(&format!("Calculate script of {target}"), &o, &mut fields);
        for (name, value) in &o.changes {
            out.set(&mut fields, name, value);
        }
        if !o.accepted || o.error.is_some() {
            continue;
        }
        // As in Acrobat, the calculated field's validate script sees the
        // new value and can turn it down.
        let mut value = o.value;
        if let Some((_, check)) = c.calculated_validate.iter().find(|(n, _)| n == target) {
            let v = run_one(&fields, check, Trigger::Validate, target, &value);
            out.note(&format!("Validate script of {target}"), &v, &mut fields);
            for (name, value) in &v.changes {
                out.set(&mut fields, name, value);
            }
            if !v.accepted {
                continue;
            }
            value = v.value;
        }
        out.set(&mut fields, target, &value);
    }
    // The target's own value is reported in `value`, not as a change,
    // unless a calculation changed it again.
    if let Some(i) = out
        .changes
        .iter()
        .position(|(n, v)| *n == c.target && *v == out.value)
    {
        out.changes.remove(i);
    }
    out
}

/// Runs [`run_commit`] in a helper process: `exe` started with `arg`,
/// which must read a commit from its input with [`decode_commit`], run it
/// with [`run_commit`] and write the outcome with [`encode_outcome`]. The
/// helper is killed if it takes longer than [`TIME_LIMIT`] per script.
pub fn run_isolated(
    exe: &std::path::Path,
    arg: &str,
    c: &Commit,
    no_window: impl FnOnce(&mut std::process::Command),
) -> CommitOutcome {
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};
    let scripts = 2 + (c.calculations.len() + c.calculated_validate.len()) as u32;
    let limit = TIME_LIMIT * scripts.min(30) + Duration::from_secs(1);
    let mut cmd = Command::new(exe);
    cmd.arg(arg)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env_clear();
    no_window(&mut cmd);
    let fail = |e: String| CommitOutcome {
        accepted: true,
        value: c.value.clone(),
        errors: vec![e],
        ..CommitOutcome::default()
    };
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return fail(format!("could not start the script helper: {e}")),
    };
    let input = encode_commit(c);
    if let Some(mut stdin) = child.stdin.take() {
        // Written from its own thread, so a helper that stops reading cannot
        // block the viewer.
        std::thread::spawn(move || {
            let _ = stdin.write_all(&input);
        });
    }
    let mut stdout = child.stdout.take();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(out) = &mut stdout {
            let _ = out.take(64 * 1024 * 1024).read_to_end(&mut bytes);
        }
        let _ = tx.send(bytes);
    });
    let bytes = match rx.recv_timeout(limit) {
        Ok(b) => b,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return fail(format!(
                "the scripts were stopped after {} seconds",
                limit.as_secs()
            ));
        }
    };
    let status = child.wait();
    match decode_outcome(&bytes) {
        Some(o) => o,
        None => fail(match status {
            Ok(s) if !s.success() => format!("the script helper stopped ({s})"),
            _ => "the script helper gave no answer".into(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(name: &str, value: &str) -> FieldValue {
        FieldValue {
            name: name.into(),
            value: value.into(),
            kind: "text",
            hidden: false,
        }
    }

    fn run_on(script: &str, trigger: Trigger, target: &str, value: &str) -> Outcome {
        run(Run {
            document_scripts: vec![],
            script: script.into(),
            trigger,
            target: target.into(),
            value: value.into(),
            fields: vec![
                field("price", "12.50"),
                field("qty", "3"),
                field("total", ""),
                field("items.a", "1"),
                field("items.b", "2,5"),
            ],
        })
    }

    #[test]
    fn commits_run_validation_then_calculations_in_order() {
        let c = Commit {
            document_scripts: vec!["function twice(v) { return v * 2; }".into()],
            fields: vec![field("qty", "1"), field("sub", ""), field("total", "")],
            target: "qty".into(),
            value: "3".into(),
            keystroke: Some("AFNumber_Keystroke(0, 0);".into()),
            validate: Some("AFRange_Validate(true, 1, true, 99);".into()),
            calculations: vec![
                (
                    "sub".into(),
                    "event.value = twice(getField('qty').value);".into(),
                ),
                (
                    "total".into(),
                    "event.value = getField('sub').value + 1;".into(),
                ),
            ],
            calculated_validate: vec![],
        };
        let o = run_commit(&c);
        assert!(o.accepted, "{o:?}");
        assert_eq!(o.value, "3");
        assert_eq!(
            o.changes,
            vec![
                ("sub".to_string(), "6".to_string()),
                ("total".to_string(), "7".to_string())
            ]
        );
        assert!(o.errors.is_empty(), "{o:?}");
        // Out of range: rejected, nothing calculated.
        let o = run_commit(&Commit {
            value: "500".into(),
            ..c.clone()
        });
        assert!(!o.accepted);
        assert!(o.changes.is_empty());
        // The wire format carries both ways.
        assert_eq!(decode_commit(&encode_commit(&c)), Some(c));
        let o = run_commit(&Commit {
            value: "2".into(),
            ..Commit::default()
        });
        assert_eq!(decode_outcome(&encode_outcome(&o)), Some(o));
    }

    #[test]
    fn calculated_values_are_validated_and_can_show_fields() {
        // The pattern of a real tax form: a total hidden while its inputs
        // are empty, shown by its own validate script once it has a value.
        let mut total = field("t.total", "");
        total.hidden = true;
        let c = Commit {
            fields: vec![field("n.a", ""), field("n.b", "4"), total],
            target: "n.a".into(),
            value: "1,5".into(),
            calculations: vec![(
                "t.total".into(),
                "AFSimple_Calculate('SUM', new Array('n.a', 'n.b'));".into(),
            )],
            calculated_validate: vec![(
                "t.total".into(),
                "event.target.hidden = getField('n.a').value == '' && \
                 getField('n.b').value == ''; if (event.value > 100) event.rc = false;"
                    .into(),
            )],
            ..Commit::default()
        };
        let o = run_commit(&c);
        assert!(o.errors.is_empty(), "{o:?}");
        assert_eq!(o.changes, vec![("t.total".to_string(), "5.5".to_string())]);
        assert_eq!(o.hidden, vec![("t.total".to_string(), false)]);
        assert_eq!(decode_commit(&encode_commit(&c)), Some(c.clone()));
        assert_eq!(decode_outcome(&encode_outcome(&o)), Some(o));
        // A calculated value its validate script turns down is not set.
        let o = run_commit(&Commit {
            value: "200".into(),
            ..c.clone()
        });
        assert!(o.changes.is_empty(), "{o:?}");
        // Hiding by parent name reaches the children; display works too.
        let o = run_on(
            "getField('items').display = display.hidden; \
             event.value = getField('items.a').hidden;",
            Trigger::Calculate,
            "total",
            "",
        );
        assert_eq!(o.shown, vec![("items".to_string(), true)]);
        assert_eq!(o.value, "true");
    }

    #[test]
    fn calculations_read_other_fields() {
        let o = run_on(
            "event.value = this.getField('price').value * getField('qty').value;",
            Trigger::Calculate,
            "total",
            "",
        );
        assert_eq!(o.error, None);
        assert_eq!(o.value, "37.5");
        let o = run_on(
            "AFSimple_Calculate('SUM', ['price', 'items']);",
            Trigger::Calculate,
            "total",
            "",
        );
        assert_eq!(o.value, "16");
    }

    #[test]
    fn scripts_can_set_fields_and_alert() {
        let o = run_on(
            "getField('qty').value = 4; app.alert({cMsg: 'Four now'});",
            Trigger::Calculate,
            "total",
            "",
        );
        assert_eq!(o.changes, vec![("qty".to_string(), "4".to_string())]);
        assert_eq!(o.alerts, vec!["Four now"]);
    }

    #[test]
    fn validation_can_reject() {
        let o = run_on(
            "AFRange_Validate(true, 0, true, 10);",
            Trigger::Validate,
            "qty",
            "12",
        );
        assert!(!o.accepted);
        assert_eq!(o.alerts.len(), 1);
        let o = run_on(
            "AFNumber_Keystroke(2, 0);",
            Trigger::Keystroke,
            "price",
            "abc",
        );
        assert!(!o.accepted);
        let o = run_on(
            "AFNumber_Format(2, 0, 0, 0, '$', true);",
            Trigger::Format,
            "price",
            "1234.5",
        );
        assert_eq!(o.value, "$1,234.50");
        let o = run_on(
            "event.value = [AFMakeNumber('1.234,56'), AFMakeNumber('1,234.56'), \
             AFMakeNumber('12,5'), AFMakeNumber('x')].join(' ');",
            Trigger::Calculate,
            "total",
            "",
        );
        assert_eq!(o.value, "1234.56 1234.56 12.5 ");
    }

    #[test]
    fn runaway_scripts_are_stopped() {
        let o = run_on("while (true) {}", Trigger::Calculate, "total", "");
        assert!(o.error.is_some());
        let o = run_on(
            "function f() { return f(); } f();",
            Trigger::Calculate,
            "total",
            "",
        );
        assert!(o.error.is_some());
    }

    #[test]
    fn nothing_outside_the_form_is_reachable() {
        // No host objects: these are all undefined in the sandbox.
        let o = run_on(
            "event.value = [typeof require, typeof process, typeof fetch, typeof XMLHttpRequest, \
             typeof this.submitForm, typeof this.exportDataObject, typeof app.launchURL, \
             typeof this.mailDoc].join(',');",
            Trigger::Calculate,
            "total",
            "",
        );
        assert_eq!(o.error, None);
        assert_eq!(
            o.value,
            "undefined,undefined,undefined,undefined,undefined,undefined,undefined,undefined"
        );
    }

    #[test]
    fn errors_are_reported_and_names_are_escaped() {
        let o = run_on("nonsense(", Trigger::Calculate, "total", "");
        assert!(o.error.is_some());
        let o = run(Run {
            document_scripts: vec![],
            script: "event.value = getField('a\"\\n\u{2028}b').value;".into(),
            trigger: Trigger::Calculate,
            target: "t".into(),
            value: String::new(),
            fields: vec![field("a\"\n\u{2028}b", "x\"y")],
        });
        assert_eq!(o.error, None);
        assert_eq!(o.value, "x\"y");
    }
}
