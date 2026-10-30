//! Form scripts: blocked unless allowed, one by one, in the inspector.
//!
//! Only the scripts that check and work out field values (keystroke on
//! commit, validate, calculate, and the document scripts they lean on) can
//! run, and only once allowed. They run in a helper process, in a sandbox
//! that sees nothing but the field values (see `atelier-script`). Every
//! other action in the file, from opening links to submitting the form,
//! is never run.

use std::collections::HashSet;

use atelier_form::{FieldKind, Form, FormScripts, Value};
use atelier_script::{Commit, CommitOutcome, FieldValue};

use super::Viewer;

/// A script, known by its text, so allowing it holds for that exact text.
pub(super) type ScriptId = [u8; 32];

pub(super) fn script_id(source: &str) -> ScriptId {
    atelier_crypto::sha256(source.as_bytes())
}

/// The form's scripts and which ones are allowed.
#[derive(Default)]
pub(super) struct ScriptState {
    pub all: FormScripts,
    pub allowed: HashSet<ScriptId>,
}

impl ScriptState {
    pub fn is_allowed(&self, source: &str) -> bool {
        self.allowed.contains(&script_id(source))
    }

    /// The scripts that can be allowed (Format scripts are listed but not
    /// run: they only change how a value looks).
    pub fn runnable(&self) -> Vec<&str> {
        let mut out: Vec<&str> = self.all.document.iter().map(|(_, s)| s.as_str()).collect();
        for f in &self.all.fields {
            for s in [&f.keystroke, &f.validate, &f.calculate]
                .into_iter()
                .flatten()
            {
                out.push(s);
            }
        }
        out
    }

    pub fn allowed_count(&self) -> usize {
        self.runnable()
            .iter()
            .filter(|s| self.is_allowed(s))
            .count()
    }

    pub fn allow_all(&mut self) {
        let ids: Vec<ScriptId> = self.runnable().iter().map(|s| script_id(s)).collect();
        self.allowed.extend(ids);
    }

    pub fn toggle(&mut self, source: &str) {
        let id = script_id(source);
        if !self.allowed.remove(&id) {
            self.allowed.insert(id);
        }
    }
}

/// Acrobat's name for a kind of field.
fn kind_name(kind: FieldKind) -> &'static str {
    match kind {
        FieldKind::Text => "text",
        FieldKind::Checkbox => "checkbox",
        FieldKind::Radio => "radiobutton",
        FieldKind::ComboBox => "combobox",
        FieldKind::ListBox => "listbox",
        FieldKind::Signature => "signature",
        _ => "button",
    }
}

fn value_text(v: &Value) -> String {
    match v {
        Value::None => String::new(),
        Value::Text(t) | Value::State(t) => t.clone(),
        Value::Choices(c) => c.first().cloned().unwrap_or_default(),
    }
}

/// The scripts that would run for a commit to `field`, or `None` when
/// none of them is allowed.
fn commit_for(
    state: &ScriptState,
    form: &Form,
    hidden: impl Fn(usize) -> bool,
    field: usize,
    value: &Value,
) -> Option<Commit> {
    let allowed = |s: &Option<String>| s.clone().filter(|s| state.is_allowed(s));
    let own = state.all.fields.get(field)?;
    let keystroke = allowed(&own.keystroke);
    let validate = allowed(&own.validate);
    let calculations: Vec<(String, String)> = state
        .all
        .calculation_order
        .iter()
        .filter_map(|&i| {
            let script = allowed(&state.all.fields.get(i)?.calculate)?;
            Some((form.fields.get(i)?.name.clone(), script))
        })
        .collect();
    let calculated_validate = state
        .all
        .calculation_order
        .iter()
        .filter_map(|&i| {
            let script = allowed(&state.all.fields.get(i)?.validate)?;
            Some((form.fields.get(i)?.name.clone(), script))
        })
        .collect();
    if keystroke.is_none() && validate.is_none() && calculations.is_empty() {
        return None;
    }
    let target = form.fields.get(field)?.name.clone();
    Some(Commit {
        document_scripts: state
            .all
            .document
            .iter()
            .filter(|(_, s)| state.is_allowed(s))
            .map(|(_, s)| s.clone())
            .collect(),
        fields: form
            .fields
            .iter()
            .enumerate()
            .map(|(i, f)| FieldValue {
                name: f.name.clone(),
                value: if i == field {
                    value_text(value)
                } else {
                    value_text(&f.value)
                },
                kind: kind_name(f.kind),
                hidden: hidden(i),
            })
            .collect(),
        value: value_text(value),
        target,
        keystroke,
        validate,
        calculations,
        calculated_validate,
    })
}

fn run(commit: &Commit) -> CommitOutcome {
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            return CommitOutcome {
                accepted: true,
                value: commit.value.clone(),
                errors: vec![format!("could not find Atelier to run the scripts: {e}")],
                ..CommitOutcome::default()
            };
        }
    };
    atelier_script::run_isolated(&exe, crate::SCRIPT_HELPER, commit, |cmd| {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW: no console flashes up.
            cmd.creation_flags(0x0800_0000);
            // Windows itself needs this to start a process.
            if let Some(root) = std::env::var_os("SystemRoot") {
                cmd.env("SystemRoot", root);
            }
        }
        #[cfg(not(windows))]
        let _ = cmd;
    })
}

/// What the scripts made of a commit.
#[derive(Default)]
pub(super) struct Scripted {
    /// The value to commit.
    pub value: Value,
    /// Other fields' new values.
    pub changes: Vec<(usize, Value)>,
    /// Fields shown (false) or hidden (true).
    pub hidden: Vec<(usize, bool)>,
}

impl Viewer {
    /// Runs the allowed scripts for a value about to be committed. Returns
    /// what to change, or `None` when a script rejected the value.
    pub(super) fn run_scripts(&mut self, field: usize, value: Value) -> Option<Scripted> {
        let o = self.open.as_ref()?;
        let fv = o.form.as_ref()?;
        let Some(commit) = o
            .scripts
            .as_ref()
            .and_then(|s| commit_for(s, &fv.form, |i| o.session.is_hidden(i), field, &value))
        else {
            return Some(Scripted {
                value,
                ..Scripted::default()
            });
        };
        let outcome = run(&commit);
        let form = &fv.form;
        let typed = |i: usize| {
            form.fields
                .get(i)
                .is_some_and(|f| matches!(f.kind, FieldKind::Text | FieldKind::ComboBox))
        };
        let mut said: Vec<String> = outcome
            .alerts
            .iter()
            .map(|a| format!("The form says: {a}"))
            .collect();
        said.extend(
            outcome
                .errors
                .iter()
                .map(|e| format!("A form script failed: {e}")),
        );
        if !outcome.accepted {
            self.error = Some(if said.is_empty() {
                "The form's script did not accept that value.".into()
            } else {
                said.join(" ")
            });
            return None;
        }
        let value = if typed(field) && outcome.value != commit.value {
            Value::Text(outcome.value.clone())
        } else {
            value
        };
        let changes = outcome
            .changes
            .iter()
            .filter_map(|(name, v)| {
                let i = form.fields.iter().position(|f| f.name == *name)?;
                (typed(i) && i != field).then(|| (i, Value::Text(v.clone())))
            })
            .collect();
        // A name can be a parent: it shows or hides every field under it.
        let mut hidden = Vec::new();
        for (name, h) in &outcome.hidden {
            let prefix = format!("{name}.");
            for (i, f) in form.fields.iter().enumerate() {
                if f.name == *name || f.name.starts_with(&prefix) {
                    hidden.retain(|(j, _)| *j != i);
                    hidden.push((i, *h));
                }
            }
        }
        if !said.is_empty() {
            self.notice = Some(said.join(" "));
        }
        Some(Scripted {
            value,
            changes,
            hidden,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_runs_until_allowed() {
        let mut state = ScriptState {
            all: FormScripts {
                document: vec![("lib".into(), "function f() {}".into())],
                fields: vec![
                    atelier_form::FieldScripts {
                        calculate: Some("event.value = 1;".into()),
                        ..Default::default()
                    },
                    Default::default(),
                ],
                calculation_order: vec![0],
            },
            allowed: HashSet::new(),
        };
        assert_eq!(state.allowed_count(), 0);
        state.toggle("event.value = 1;");
        assert_eq!(state.allowed_count(), 1);
        state.allow_all();
        assert_eq!(state.allowed_count(), 2);
        // A changed script is a different script, and not allowed.
        assert!(!state.is_allowed("event.value = 2;"));
    }
}
