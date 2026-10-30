//! What a script would ask of the viewer, judged from its text alone: the
//! Acrobat features it leans on, and the things worth a second look before
//! anyone opens the file elsewhere. Nothing is run or parsed as code; this
//! is a search for known names, so it can be fooled and can over-report.

/// Something a script uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ScriptTrait {
    /// Asks which viewer, version or platform it runs in.
    ViewerCheck,
    /// Uses the XFA object model, which only Adobe viewers have.
    XfaModel,
    /// Acrobat's dialogs, menus or toolbars.
    AcrobatUi,
    /// Acrobat's built-in format and calculate helpers (AFNumber_Format...).
    AcrobatHelpers,
    /// Sends data out or opens a web address.
    Network,
    /// Opens, saves, imports or launches files, embedded files included.
    Files,
    /// Asks for privileged, trusted or plug-in features.
    Privileged,
    /// Prints.
    Printing,
    /// Builds or unpacks code while running: eval, unescape and the like.
    Obfuscated,
}

impl ScriptTrait {
    pub fn label(self) -> &'static str {
        match self {
            Self::ViewerCheck => "checks which viewer it runs in",
            Self::XfaModel => "uses the XFA object model",
            Self::AcrobatUi => "uses Acrobat dialogs or menus",
            Self::AcrobatHelpers => "uses Acrobat's built-in format functions",
            Self::Network => "sends data or opens web addresses",
            Self::Files => "opens, saves or launches files",
            Self::Privileged => "asks for privileged features",
            Self::Printing => "prints",
            Self::Obfuscated => "builds code at run time",
        }
    }

    /// Behaves differently, or only works, in some viewers.
    pub fn is_viewer_dependent(self) -> bool {
        matches!(
            self,
            Self::ViewerCheck | Self::XfaModel | Self::AcrobatUi | Self::AcrobatHelpers
        ) || self.needs_care()
    }

    /// Worth a look before the file is opened in a viewer that runs scripts.
    pub fn needs_care(self) -> bool {
        matches!(
            self,
            Self::Network | Self::Files | Self::Privileged | Self::Obfuscated
        )
    }
}

/// Names to look for, and what each one means. A name ending in '.' or
/// '(' matches as written; others must not run on into an identifier.
const PATTERNS: &[(&str, ScriptTrait)] = &[
    ("app.viewerType", ScriptTrait::ViewerCheck),
    ("app.viewerVariation", ScriptTrait::ViewerCheck),
    ("app.viewerVersion", ScriptTrait::ViewerCheck),
    ("app.platform", ScriptTrait::ViewerCheck),
    ("app.plugIns", ScriptTrait::ViewerCheck),
    ("xfa.host", ScriptTrait::ViewerCheck),
    ("xfa.", ScriptTrait::XfaModel),
    ("app.alert", ScriptTrait::AcrobatUi),
    ("app.response", ScriptTrait::AcrobatUi),
    ("app.execDialog", ScriptTrait::AcrobatUi),
    ("app.execMenuItem", ScriptTrait::AcrobatUi),
    ("app.addMenuItem", ScriptTrait::AcrobatUi),
    ("app.addSubMenu", ScriptTrait::AcrobatUi),
    ("app.addToolButton", ScriptTrait::AcrobatUi),
    ("app.popUpMenu", ScriptTrait::AcrobatUi),
    ("app.setTimeOut", ScriptTrait::AcrobatUi),
    ("app.setInterval", ScriptTrait::AcrobatUi),
    ("AFNumber_", ScriptTrait::AcrobatHelpers),
    ("AFPercent_", ScriptTrait::AcrobatHelpers),
    ("AFDate_", ScriptTrait::AcrobatHelpers),
    ("AFTime_", ScriptTrait::AcrobatHelpers),
    ("AFSpecial_", ScriptTrait::AcrobatHelpers),
    ("AFSimple_", ScriptTrait::AcrobatHelpers),
    ("AFRange_", ScriptTrait::AcrobatHelpers),
    ("AFMergeChange", ScriptTrait::AcrobatHelpers),
    ("submitForm", ScriptTrait::Network),
    ("mailDoc", ScriptTrait::Network),
    ("mailForm", ScriptTrait::Network),
    ("mailMsg", ScriptTrait::Network),
    ("launchURL", ScriptTrait::Network),
    ("getURL", ScriptTrait::Network),
    ("SOAP.", ScriptTrait::Network),
    ("Net.HTTP", ScriptTrait::Network),
    ("Net.SOAP", ScriptTrait::Network),
    ("Net.Discovery", ScriptTrait::Network),
    ("RSS.", ScriptTrait::Network),
    ("exportDataObject", ScriptTrait::Files),
    ("importDataObject", ScriptTrait::Files),
    ("openDataObject", ScriptTrait::Files),
    ("createDataObject", ScriptTrait::Files),
    ("getDataObjectContents", ScriptTrait::Files),
    ("importAnFDF", ScriptTrait::Files),
    ("importAnXFDF", ScriptTrait::Files),
    ("importTextData", ScriptTrait::Files),
    ("importXFAData", ScriptTrait::Files),
    ("exportAsFDF", ScriptTrait::Files),
    ("exportAsXFDF", ScriptTrait::Files),
    ("exportAsText", ScriptTrait::Files),
    ("saveAs", ScriptTrait::Files),
    ("app.openDoc", ScriptTrait::Files),
    ("app.newDoc", ScriptTrait::Files),
    ("app.browseForDoc", ScriptTrait::Files),
    ("readFileIntoStream", ScriptTrait::Files),
    ("app.launch", ScriptTrait::Files),
    ("app.trustedFunction", ScriptTrait::Privileged),
    ("app.trustPropagatorFunction", ScriptTrait::Privileged),
    ("app.beginPriv", ScriptTrait::Privileged),
    ("security.", ScriptTrait::Privileged),
    ("Collab.", ScriptTrait::Privileged),
    ("ADBC.", ScriptTrait::Privileged),
    ("identity.", ScriptTrait::Privileged),
    ("app.media", ScriptTrait::Privileged),
    ("print(", ScriptTrait::Printing),
    ("printWithParams", ScriptTrait::Printing),
    ("eval(", ScriptTrait::Obfuscated),
    ("unescape(", ScriptTrait::Obfuscated),
    ("fromCharCode", ScriptTrait::Obfuscated),
    ("Function(", ScriptTrait::Obfuscated),
    ("atob(", ScriptTrait::Obfuscated),
];

/// Longer scripts are only searched this far.
const MAX_SCAN: usize = 4 << 20;

/// What a script uses, each with the first name that showed it, in the
/// order of [`ScriptTrait`].
pub fn script_traits(source: &str) -> Vec<(ScriptTrait, &'static str)> {
    let mut end = source.len().min(MAX_SCAN);
    while !source.is_char_boundary(end) {
        end -= 1;
    }
    let text = &source[..end];
    let mut found: Vec<(ScriptTrait, &'static str)> = Vec::new();
    for &(pattern, t) in PATTERNS {
        if found.iter().any(|(f, _)| *f == t) {
            continue;
        }
        if contains_name(text, pattern) {
            found.push((t, pattern));
        }
    }
    found.sort();
    found
}

/// Whether `name` occurs in `text` as a name of its own: not the tail of a
/// longer identifier, and (unless it ends in punctuation) not the start
/// of one.
fn contains_name(text: &str, name: &str) -> bool {
    let ident = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    let open_end = name.ends_with(['.', '(', '_']);
    let mut from = 0;
    while let Some(i) = text[from..].find(name) {
        let at = from + i;
        let before = text[..at].chars().next_back();
        let after = text[at + name.len()..].chars().next();
        // "this.print(" and "app.alert" count, "myprint(" does not.
        let starts_clean = before.is_none_or(|c| !ident(c));
        let ends_clean = open_end || after.is_none_or(|c| !ident(c));
        if starts_clean && ends_clean {
            return true;
        }
        from = at + name.len();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn traits(s: &str) -> Vec<ScriptTrait> {
        script_traits(s).into_iter().map(|(t, _)| t).collect()
    }

    #[test]
    fn finds_viewer_checks_and_risky_calls() {
        assert_eq!(
            traits(
                "if (app.viewerType == 'Reader') this.exportDataObject({cName:'a', nLaunch:2});"
            ),
            vec![ScriptTrait::ViewerCheck, ScriptTrait::Files]
        );
        assert_eq!(
            traits("eval(unescape('%61'))"),
            vec![ScriptTrait::Obfuscated]
        );
        assert_eq!(
            traits("AFNumber_Format(2, 0, 0, 0, '', true);"),
            vec![ScriptTrait::AcrobatHelpers]
        );
        assert_eq!(
            traits("xfa.resolveNode('a').rawValue = 1; this.print();"),
            vec![ScriptTrait::XfaModel, ScriptTrait::Printing]
        );
    }

    #[test]
    fn ignores_names_inside_other_names() {
        assert!(traits("var myprint = 1; reprint(2); doSaveAsThing();").is_empty());
        assert!(traits("event.value = this.getField('a').value * 2;").is_empty());
    }

    #[test]
    fn checks_are_marked_viewer_dependent() {
        assert!(ScriptTrait::ViewerCheck.is_viewer_dependent());
        assert!(!ScriptTrait::Printing.is_viewer_dependent());
        assert!(ScriptTrait::Network.needs_care());
    }
}
