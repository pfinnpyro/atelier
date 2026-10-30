//! Recognising XFA forms (ISO 32000-1, 12.7.8; XFA 3.3). Atelier does not
//! draw XFA: it reads what kind of XFA a file has, so the viewer can say
//! what it will and will not show, and it lists the XFA scripts so they
//! can be inspected. The XFA itself is never changed.

use atelier_syntax::{Document, Object};

use crate::xml::{Reader, Token, attr};

/// What a file's XFA form is.
#[derive(Debug, Clone, PartialEq)]
pub struct Xfa {
    /// The packets, in file order, with their decoded sizes in bytes
    /// (0 when the XFA is one stream and sizes are not known).
    pub packets: Vec<(String, usize)>,
    /// The pages are laid out by the XFA form when it is opened, so the
    /// PDF pages hold a stand-in (often a "please wait" page) rather than
    /// the form. Static XFA forms keep a full AcroForm copy instead.
    pub dynamic: bool,
    /// Why it is judged dynamic or static, for people.
    pub reasons: Vec<String>,
    /// The template has subforms that flow (grow with their content).
    pub flowing: bool,
    /// Scripts in the template. None of them are run.
    pub scripts: Vec<XfaScript>,
}

/// A script in the XFA template.
#[derive(Debug, Clone, PartialEq)]
pub struct XfaScript {
    /// The form object it belongs to and the event that would run it:
    /// "form1.page1.total, calculate".
    pub location: String,
    /// "JavaScript" or "FormCalc".
    pub language: String,
    /// Where it would run: "client", "server" or "both".
    pub run_at: Option<String>,
    pub source: String,
}

/// More XFA than this is not read.
const MAX_XFA: usize = 64 << 20;
const MAX_SCRIPTS: usize = 10_000;
const MAX_DEPTH: usize = 512;

/// The document's XFA form, or None when it has none.
pub fn xfa(doc: &Document) -> Option<Xfa> {
    let catalog = doc.catalog().ok()?;
    let Ok(Object::Dict(acro)) = doc.get_key(&catalog, b"AcroForm") else {
        return None;
    };
    let entry = doc.get_key(&acro, b"XFA").ok().filter(|o| !o.is_null())?;

    // Either one stream holding the whole XDP document, or an array of
    // packet names and streams that join up into one.
    let mut packets = Vec::new();
    let mut xdp = Vec::new();
    match entry {
        Object::Stream(s) => {
            if let Ok(d) = doc.decode_stream(&s) {
                xdp = d.data;
                xdp.truncate(MAX_XFA);
            }
        }
        Object::Array(items) => {
            for pair in items.chunks(2) {
                let name = match pair.first().map(|o| doc.resolve(o)) {
                    Some(Ok(Object::String(s))) => s.to_text(),
                    _ => String::new(),
                };
                let data = match pair.get(1).map(|o| doc.resolve(o)) {
                    Some(Ok(Object::Stream(s))) => {
                        doc.decode_stream(&s).map(|d| d.data).unwrap_or_default()
                    }
                    _ => Vec::new(),
                };
                if xdp.len() + data.len() > MAX_XFA {
                    break;
                }
                packets.push((name, data.len()));
                xdp.extend_from_slice(&data);
            }
        }
        _ => {}
    }

    let read = read_xdp(&xdp);
    if packets.is_empty() {
        packets = read.packets;
    }

    let needs_rendering = doc
        .get_key(&catalog, b"NeedsRendering")
        .ok()
        .and_then(|o| o.as_bool())
        .unwrap_or(false);
    let has_fields = doc
        .get_key(&acro, b"Fields")
        .ok()
        .and_then(|o| o.as_array().map(|a| !a.is_empty()))
        .unwrap_or(false);

    let mut reasons = Vec::new();
    if needs_rendering {
        reasons.push("The file asks the viewer to build its pages from the XFA form.".into());
    }
    if read.dynamic_render {
        reasons.push("The XFA configuration requires dynamic rendering.".into());
    }
    if !has_fields {
        reasons
            .push("The AcroForm has no fields, so the PDF pages hold no copy of the form.".into());
    }
    let dynamic = !reasons.is_empty();
    if dynamic {
        if read.flowing {
            reasons.push("Its subforms flow with their content.".into());
        }
    } else {
        reasons.push(
            "The PDF pages and AcroForm fields carry the form (static XFA), so Atelier can show and fill it."
                .into(),
        );
    }
    Some(Xfa {
        packets,
        dynamic,
        reasons,
        flowing: read.flowing,
        scripts: read.scripts,
    })
}

#[derive(Default)]
struct Read {
    packets: Vec<(String, usize)>,
    dynamic_render: bool,
    flowing: bool,
    scripts: Vec<XfaScript>,
}

/// Elements whose name attribute is part of a script's location.
const CONTAINERS: &[&str] = &[
    "subform",
    "subformSet",
    "field",
    "exclGroup",
    "draw",
    "pageSet",
    "pageArea",
    "area",
    "variables",
];

struct Open {
    name: String,
    /// The name attribute, for containers.
    label: Option<String>,
    /// For events: what sets them off.
    activity: Option<String>,
}

fn read_xdp(xdp: &[u8]) -> Read {
    let mut read = Read::default();
    let mut stack: Vec<Open> = Vec::new();
    // The packet being read, as the top-level element under xdp:xdp.
    let mut packet: Option<String> = None;
    // The script being read: its location, language, run-at and text.
    let mut script: Option<(String, String, Option<String>, String)> = None;
    let mut dynamic_render = false;
    let mut text = String::new();
    let mut seen_root = false;

    for token in Reader::new(xdp) {
        match token {
            Token::Start { name, attrs, empty } => {
                if stack.len() >= MAX_DEPTH {
                    continue;
                }
                // With no xdp wrapper, the first element is the packet.
                let depth_of_packet = if seen_root || name == "xdp" { 1 } else { 0 };
                if name == "xdp" && stack.is_empty() {
                    seen_root = true;
                }
                if stack.len() == depth_of_packet && name != "xdp" {
                    if empty {
                        read.packets.push((name.clone(), 0));
                        continue;
                    }
                    packet = Some(name.clone());
                }
                let in_template = packet.as_deref() == Some("template");
                if in_template && name == "subform" {
                    if let Some(layout) = attr(&attrs, "layout") {
                        if matches!(layout, "tb" | "lr-tb" | "rl-tb" | "table" | "row") {
                            read.flowing = true;
                        }
                    }
                }
                if in_template && name == "script" && script.is_none() {
                    let language = match attr(&attrs, "contentType") {
                        Some(t) if t.contains("javascript") => "JavaScript",
                        _ => "FormCalc",
                    };
                    let mut location = location_of(&stack);
                    if let Some(n) = attr(&attrs, "name") {
                        location = format!("{location}, script object {n}");
                    }
                    script = Some((
                        location,
                        language.to_string(),
                        attr(&attrs, "runAt").map(str::to_string),
                        String::new(),
                    ));
                }
                if !empty {
                    let label = CONTAINERS
                        .contains(&name.as_str())
                        .then(|| attr(&attrs, "name").map(str::to_string))
                        .flatten();
                    let activity = (name == "event")
                        .then(|| attr(&attrs, "activity").map(str::to_string))
                        .flatten();
                    stack.push(Open {
                        name,
                        label,
                        activity,
                    });
                    text.clear();
                }
            }
            Token::Text(t) => {
                if let Some((_, _, _, src)) = &mut script {
                    src.push_str(&t);
                } else if text.len() < 256 {
                    text.push_str(&t);
                }
            }
            Token::End { name } => {
                if name == "dynamicRender"
                    && packet.as_deref() == Some("config")
                    && text.trim() == "required"
                {
                    dynamic_render = true;
                }
                if name == "script" {
                    if let Some((location, language, run_at, source)) = script.take() {
                        if read.scripts.len() < MAX_SCRIPTS && !source.trim().is_empty() {
                            read.scripts.push(XfaScript {
                                location,
                                language,
                                run_at,
                                source: source.trim().to_string(),
                            });
                        }
                    }
                }
                // Close up to the matching element, forgiving missing ends.
                if let Some(i) = stack.iter().rposition(|o| o.name == name) {
                    stack.truncate(i);
                }
                let depth_of_packet = usize::from(seen_root);
                if stack.len() == depth_of_packet {
                    if let Some(p) = packet.take() {
                        read.packets.push((p, 0));
                    }
                }
                text.clear();
            }
        }
    }
    read.dynamic_render = dynamic_render;
    read
}

/// "form1.page1.total, calculate" from the open elements.
fn location_of(stack: &[Open]) -> String {
    let path: Vec<&str> = stack.iter().filter_map(|o| o.label.as_deref()).collect();
    let path = if path.is_empty() {
        "form".to_string()
    } else {
        path.join(".")
    };
    let event = stack.iter().rev().find_map(|o| match o.name.as_str() {
        "event" => Some(o.activity.clone().unwrap_or_else(|| "event".into())),
        "calculate" => Some("calculate".into()),
        "validate" => Some("validate".into()),
        "variables" => Some("variables".into()),
        _ => None,
    });
    match event {
        Some(e) => format!("{path}, {e}"),
        None => path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const XDP: &str = r#"<?xml version="1.0"?>
<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<config><present><script><runScripts>both</runScripts></script></present>
<acrobat><acrobat7><dynamicRender>required</dynamicRender></acrobat7></acrobat></config>
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form1" layout="tb">
 <field name="total">
  <calculate><script contentType="application/x-javascript">this.rawValue = 1 &lt; 2;</script></calculate>
  <event activity="click"><script contentType="application/x-javascript" runAt="client"><![CDATA[app.alert("hi")]]></script></event>
 </field>
 <variables><script name="util" contentType="application/x-javascript">function f(){}</script></variables>
 <field name="fc"><event activity="exit"><script>$ = 1</script></event></field>
</subform>
</template>
<datasets/>
</xdp:xdp>"#;

    #[test]
    fn reads_packets_scripts_and_dynamic_render() {
        let r = read_xdp(XDP.as_bytes());
        let names: Vec<&str> = r.packets.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["config", "template", "datasets"]);
        assert!(r.dynamic_render);
        assert!(r.flowing);
        let s: Vec<(&str, &str, &str)> = r
            .scripts
            .iter()
            .map(|s| (s.location.as_str(), s.language.as_str(), s.source.as_str()))
            .collect();
        assert_eq!(
            s,
            [
                (
                    "form1.total, calculate",
                    "JavaScript",
                    "this.rawValue = 1 < 2;"
                ),
                ("form1.total, click", "JavaScript", "app.alert(\"hi\")"),
                (
                    "form1, variables, script object util",
                    "JavaScript",
                    "function f(){}"
                ),
                ("form1.fc, exit", "FormCalc", "$ = 1"),
            ]
        );
        // The config packet's <script> element is not a script.
        assert_eq!(r.scripts[1].run_at.as_deref(), Some("client"));
    }
}
