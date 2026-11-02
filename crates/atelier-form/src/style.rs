//! Editable field properties. Each property has a separate revision target.
use crate::{DefaultAppearance, FieldKind, FillWarning, Form, Session};
use atelier_syntax::{ContentParser, Error, Name, Object, PdfString, Rect, Result, Update};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq)]
pub struct FieldStyle {
    pub appearance: DefaultAppearance,
    /// 0 left, 1 center, 2 right.
    pub alignment: u8,
}

fn invalid(message: &str) -> Error {
    Error::Structure(message.into())
}

impl Session {
    pub fn field_style(&self, field: usize) -> Option<FieldStyle> {
        if let Some(style) = self.styles.get(&field) {
            return Some(style.clone());
        }
        let form = self.original()?;
        let f = form.fields.get(field)?;
        Some(FieldStyle {
            appearance: DefaultAppearance::parse(
                f.da.as_deref().or(form.da.as_deref()).unwrap_or(""),
            ),
            alignment: f.quadding,
        })
    }

    /// Validate and record an appearance change without changing the field value.
    pub fn set_field_style(&mut self, field: usize, style: FieldStyle) -> Result<()> {
        if self.field_style(field).as_ref() == Some(&style) {
            return Ok(());
        }
        let form = self.original().ok_or_else(|| invalid("No form"))?;
        let f = form
            .fields
            .get(field)
            .ok_or_else(|| invalid("Unknown field"))?;
        if !matches!(
            f.kind,
            FieldKind::Text | FieldKind::ComboBox | FieldKind::ListBox
        ) || f.is_read_only()
        {
            return Err(invalid("This field's text style cannot be edited"));
        }
        let da = &style.appearance;
        if style.alignment > 2
            || !da.size.is_finite()
            || !(0.0..=1000.0).contains(&da.size)
            || !da.leading.is_finite()
            || !(0.0..=10000.0).contains(&da.leading)
            || !da.character_spacing.is_finite()
            || da.character_spacing.abs() > 1000.0
            || !da.word_spacing.is_finite()
            || da.word_spacing.abs() > 1000.0
        {
            return Err(invalid("Invalid text size, alignment or spacing"));
        }
        let ops: Vec<_> = ContentParser::new(da.color.as_bytes()).collect();
        if ops.len() != 1
            || !matches!(
                (ops[0].operator, ops[0].operands.len()),
                (b"g", 1) | (b"rg", 3) | (b"k", 4)
            )
            || !ops[0].operands.iter().all(|o| {
                o.as_f64()
                    .is_some_and(|n| n.is_finite() && (0.0..=1.0).contains(&n))
            })
        {
            return Err(invalid("Color must be a PDF gray, RGB or CMYK fill color"));
        }
        if let Some(font) = &da.font {
            let fonts = self
                .original()
                .unwrap()
                .resources
                .get(b"Font")
                .map(|o| self.base_document().resolve_dict(o))
                .transpose()?;
            if !fonts.is_some_and(|fonts| fonts.contains_key(font.as_slice())) {
                return Err(invalid(
                    "Choose a font resource already present in this form",
                ));
            }
        }
        let before = self.field_style(field).unwrap();
        let identity = format!("Field: {} ({})", f.name, f.obj);
        let page = self.pages_of(field).first().copied();
        let values = |s: &FieldStyle| {
            [
                s.appearance
                    .font
                    .as_ref()
                    .map(|f| Name(f.clone()).to_string())
                    .unwrap_or_default(),
                s.appearance.size.to_string(),
                s.appearance.color.clone(),
                s.alignment.to_string(),
                s.appearance.character_spacing.to_string(),
                s.appearance.word_spacing.to_string(),
                s.appearance.leading.to_string(),
            ]
        };
        for ((property, before), after) in [
            "font",
            "font-size",
            "color",
            "alignment",
            "character-spacing",
            "word-spacing",
            "line-spacing",
        ]
        .into_iter()
        .zip(values(&before))
        .zip(values(&style))
        {
            if before != after {
                self.touch(format!("{identity} / {property}"), before, after, page);
            }
        }
        self.styles.insert(field, style);
        Ok(())
    }

    pub fn widget_bounds(&self, field: usize, widget: usize) -> Option<Rect> {
        self.bounds.get(&(field, widget)).copied().or_else(|| {
            self.original()?
                .fields
                .get(field)?
                .widgets
                .get(widget)
                .map(|w| w.rect)
        })
    }

    pub fn set_widget_bounds(&mut self, field: usize, widget: usize, rect: Rect) -> Result<()> {
        let f = self
            .original()
            .and_then(|f| f.fields.get(field))
            .ok_or_else(|| invalid("Unknown field"))?;
        if f.is_read_only() {
            return Err(invalid("Read-only field"));
        }
        let before = self
            .widget_bounds(field, widget)
            .ok_or_else(|| invalid("Unknown widget"))?;
        if before == rect {
            return Ok(());
        }
        if ![rect.x0, rect.y0, rect.x1, rect.y1]
            .iter()
            .all(|n| n.is_finite() && n.abs() <= 1e7)
            || rect.x1 <= rect.x0
            || rect.y1 <= rect.y0
        {
            return Err(invalid(
                "Widget bounds must be finite with positive width and height",
            ));
        }
        let identity = format!("Field: {} ({}) / widget-{widget}", f.name, f.obj);
        let page = self.pages_of(field).first().copied();
        for (property, old, new) in [
            ("x", before.x0, rect.x0),
            ("y", before.y0, rect.y0),
            ("width", before.x1 - before.x0, rect.x1 - rect.x0),
            ("height", before.y1 - before.y0, rect.y1 - rect.y0),
        ] {
            if old != new {
                self.touch(
                    format!("{identity} / {property}"),
                    old.to_string(),
                    new.to_string(),
                    page,
                );
            }
        }
        self.bounds.insert((field, widget), rect);
        Ok(())
    }

    pub(crate) fn apply_styles(
        &self,
        form: &mut Form,
        update: &mut Update<'_>,
        warnings: &mut Vec<FillWarning>,
    ) -> Result<()> {
        let mut refresh = BTreeSet::new();
        for (&index, style) in &self.styles {
            let f = &mut form.fields[index];
            let da = &style.appearance;
            let font = da
                .font
                .as_ref()
                .map(|f| Name(f.clone()).to_string())
                .unwrap_or_else(|| "/Helv".into());
            let text = format!(
                "{font} {} Tf {} {} Tc {} Tw {} TL",
                da.size, da.color, da.character_spacing, da.word_spacing, da.leading
            );
            f.da = Some(text.clone());
            f.quadding = style.alignment;
            if let Object::Dict(mut d) = update.get(f.obj)? {
                d.insert("DA", Object::String(PdfString::from_text(&text)));
                d.insert("Q", Object::Integer(style.alignment.into()));
                // Widget overrides take precedence over the field's inherited DA.
                update.set(f.obj, Object::Dict(d));
            }
            for w in &f.widgets {
                if let Object::Dict(mut d) = update.get(w.obj)? {
                    d.insert("DA", Object::String(PdfString::from_text(&text)));
                    d.insert("Q", Object::Integer(style.alignment.into()));
                    update.set(w.obj, Object::Dict(d));
                }
            }
            refresh.insert(index);
        }
        for (&(field, widget), &rect) in &self.bounds {
            let w = &mut form.fields[field].widgets[widget];
            w.rect = rect;
            if let Object::Dict(mut d) = update.get(w.obj)? {
                d.insert(
                    "Rect",
                    Object::Array(
                        [rect.x0, rect.y0, rect.x1, rect.y1]
                            .into_iter()
                            .map(Object::Real)
                            .collect(),
                    ),
                );
                update.set(w.obj, Object::Dict(d));
            }
            refresh.insert(field);
        }
        for field in refresh {
            warnings.extend(form.refresh_appearance(update, field)?);
        }
        Ok(())
    }
}
