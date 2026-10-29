//! Setting field values: the new /V, the widgets' states, and fresh
//! appearance streams, all recorded in an [`Update`].

use atelier_syntax::{Dict, Name, Object, PdfString, Result, Update};

use crate::appearance::{
    Content, Frame, button_appearance, choice_content, choose_font, text_appearance,
};
use crate::da::DefaultAppearance;
use crate::{FieldKind, Form, Value, flags};

/// Something about a fill that a person may want to know.
#[derive(Debug, Clone, PartialEq)]
pub enum FillWarning {
    /// The field's font lacks some characters of the value; they show as `?`.
    MissingCharacters { field: String },
    /// The value was cut to the field's /MaxLen.
    Truncated { field: String },
    /// The field kind cannot take this kind of value.
    WrongKind { field: String },
}

impl std::fmt::Display for FillWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingCharacters { field } => {
                write!(f, "{field}: the font lacks some characters; they show as ?")
            }
            Self::Truncated { field } => {
                write!(f, "{field}: value cut to the field's maximum length")
            }
            Self::WrongKind { field } => {
                write!(f, "{field}: this kind of field cannot take that value")
            }
        }
    }
}

impl Form {
    /// Sets a field's value and redraws its widgets. Text fields and combo
    /// boxes take [`Value::Text`]; checkboxes and radios [`Value::State`]
    /// (or [`Value::None`] for off); list boxes [`Value::Choices`].
    pub fn set_value(
        &mut self,
        update: &mut Update<'_>,
        index: usize,
        value: Value,
    ) -> Result<Vec<FillWarning>> {
        let mut warnings = Vec::new();
        let field = &self.fields[index];
        let name = field.name.clone();
        let value = match (field.kind, value) {
            (FieldKind::Text, Value::Text(t)) => {
                let t = match field.max_len {
                    Some(m) if t.chars().count() > m => {
                        warnings.push(FillWarning::Truncated {
                            field: name.clone(),
                        });
                        t.chars().take(m).collect()
                    }
                    _ => t,
                };
                Value::Text(t)
            }
            (FieldKind::ComboBox, Value::Text(t)) => Value::Text(t),
            (FieldKind::ComboBox | FieldKind::ListBox, Value::Choices(mut c)) => {
                if field.kind == FieldKind::ComboBox || !field.has_flag(flags::MULTI_SELECT) {
                    c.truncate(1);
                }
                Value::Choices(c)
            }
            (FieldKind::ListBox, Value::Text(t)) => Value::Choices(vec![t]),
            (FieldKind::Checkbox | FieldKind::Radio, Value::State(s)) if s != "Off" => {
                Value::State(s)
            }
            (FieldKind::Checkbox | FieldKind::Radio, Value::State(_) | Value::None) => {
                Value::State("Off".to_string())
            }
            (_, Value::None) => Value::None,
            _ => {
                warnings.push(FillWarning::WrongKind { field: name });
                return Ok(warnings);
            }
        };

        // The field dictionary's /V (and /I for list boxes).
        let mut dict = dict_in(update, field.obj)?;
        match &value {
            Value::None => {
                dict.remove(b"V");
            }
            Value::Text(t) => dict.insert("V", Object::String(PdfString::from_text(t))),
            Value::State(s) => dict.insert("V", Object::Name(Name::new(s.as_bytes()))),
            Value::Choices(c) => {
                let strings: Vec<Object> = c
                    .iter()
                    .map(|v| Object::String(PdfString::from_text(v)))
                    .collect();
                match strings.len() {
                    0 => {
                        dict.remove(b"V");
                    }
                    1 => dict.insert("V", strings[0].clone()),
                    _ => dict.insert("V", Object::Array(strings)),
                }
                if field.kind == FieldKind::ListBox {
                    let mut indices: Vec<i64> = field
                        .options
                        .iter()
                        .enumerate()
                        .filter(|(_, o)| c.iter().any(|v| *v == o.export || *v == o.display))
                        .map(|(i, _)| i as i64)
                        .collect();
                    indices.sort_unstable();
                    if indices.is_empty() {
                        dict.remove(b"I");
                    } else {
                        dict.insert(
                            "I",
                            Object::Array(indices.into_iter().map(Object::Integer).collect()),
                        );
                    }
                }
            }
        }
        // A rich text value would now disagree with /V.
        dict.remove(b"RV");
        update.set(field.obj, Object::Dict(dict));
        self.fields[index].value = value;
        warnings.extend(self.refresh_appearance(update, index)?);
        Ok(warnings)
    }

    /// Sets a field back to its default value (/DV).
    pub fn reset(&mut self, update: &mut Update<'_>, index: usize) -> Result<Vec<FillWarning>> {
        let default = self.fields[index].default_value.clone();
        let default = match (self.fields[index].kind, default) {
            (FieldKind::ListBox, Value::Text(t)) => Value::Choices(vec![t]),
            (_, v) => v,
        };
        self.set_value(update, index, default)
    }

    /// Redraws every widget of a field from its current value: what
    /// viewers do for files that set /NeedAppearances.
    pub fn refresh_appearance(
        &self,
        update: &mut Update<'_>,
        index: usize,
    ) -> Result<Vec<FillWarning>> {
        let doc = update.document();
        let field = &self.fields[index];
        let da = DefaultAppearance::parse(field.da.as_deref().or(self.da.as_deref()).unwrap_or(""));
        let mut lossy = false;
        for widget in &field.widgets {
            let mut dict = dict_in(update, widget.obj)?;
            let frame = Frame::new(doc, &dict, widget);
            match field.kind {
                FieldKind::Text | FieldKind::ComboBox => {
                    let text = match field.kind {
                        FieldKind::Text => field.value.display(),
                        _ => choice_content(field).0,
                    };
                    let chosen = choose_font(doc, &self.resources, &da, &[&text]);
                    let ap = text_appearance(
                        &frame,
                        field,
                        &da,
                        &chosen,
                        &Content::Text(&text),
                        &mut lossy,
                    );
                    set_normal_appearance(update, &mut dict, ap.into_stream());
                }
                FieldKind::ListBox => {
                    let (_, options) = choice_content(field);
                    let texts: Vec<&str> = options.iter().map(|(t, _)| t.as_str()).collect();
                    let chosen = choose_font(doc, &self.resources, &da, &texts);
                    let list = options.iter().map(|(t, s)| (t.as_str(), *s)).collect();
                    let ap = text_appearance(
                        &frame,
                        field,
                        &da,
                        &chosen,
                        &Content::List(list),
                        &mut lossy,
                    );
                    set_normal_appearance(update, &mut dict, ap.into_stream());
                }
                FieldKind::Checkbox | FieldKind::Radio => {
                    let state = match &field.value {
                        Value::State(s) => s.clone(),
                        _ => "Off".to_string(),
                    };
                    let on = widget.on_state.clone();
                    let shows = on.as_deref() == Some(state.as_str()) && state != "Off";
                    if on.is_none() {
                        // No appearance to switch between: draw one.
                        let on_name = if state != "Off" {
                            state.clone()
                        } else {
                            "Yes".to_string()
                        };
                        let (on_ap, off_ap) = button_appearance(
                            doc,
                            &frame,
                            &dict,
                            &da,
                            field.kind == FieldKind::Radio,
                        );
                        let mut states = Dict::new();
                        states.insert(
                            Name::new(on_name.as_bytes()),
                            Object::Reference(update.add(on_ap.into_stream())),
                        );
                        states.insert("Off", Object::Reference(update.add(off_ap.into_stream())));
                        let mut ap = Dict::new();
                        ap.insert("N", Object::Dict(states));
                        dict.insert("AP", Object::Dict(ap));
                        let as_name = if state != "Off" {
                            on_name
                        } else {
                            "Off".to_string()
                        };
                        dict.insert("AS", Object::Name(Name::new(as_name.as_bytes())));
                    } else {
                        let as_name = if shows { state } else { "Off".to_string() };
                        dict.insert("AS", Object::Name(Name::new(as_name.as_bytes())));
                    }
                    update.set(widget.obj, Object::Dict(dict));
                    continue;
                }
                FieldKind::PushButton | FieldKind::Signature | FieldKind::Unknown => continue,
            }
            update.set(widget.obj, Object::Dict(dict));
        }
        Ok(if lossy {
            vec![FillWarning::MissingCharacters {
                field: field.name.clone(),
            }]
        } else {
            Vec::new()
        })
    }
}

fn dict_in(update: &Update<'_>, r: atelier_syntax::ObjRef) -> Result<Dict> {
    match update.get(r)? {
        Object::Dict(d) => Ok(d),
        other => Err(atelier_syntax::Error::WrongType {
            expected: "dictionary",
            found: other.type_name(),
        }),
    }
}

/// Points the widget's /AP at a new normal appearance. Down and rollover
/// appearances would show the old value, so they go.
fn set_normal_appearance(update: &mut Update<'_>, dict: &mut Dict, stream: Object) {
    let r = update.add(stream);
    let mut ap = Dict::new();
    ap.insert("N", Object::Reference(r));
    dict.insert("AP", Object::Dict(ap));
}
