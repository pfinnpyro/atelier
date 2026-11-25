# Atelier 1.2.3 beta 1

This preview fixes Secure Redaction exports that stopped when a page declared
a transparency group or custom color resources.

- Vector export rebuilds supported page transparency settings, blend modes,
  and opacity. It resolves retained paints from default and named solid color
  spaces and omits source palettes and profile metadata.
- Atelier reopens and verifies the new PDF before saving. Selectable text
  and vectors on supported pages remain available outside the marks.
- Graphics-state soft masks and other unsupported effects still require
  **Export page images**.
