Source: gpui-pre 0.3.8 from crates.io (Apache-2.0). Used by gpui-component 0.7.1.

Local changes: apply Unicode line-break properties to Chinese punctuation in
both character-width measurement (including line clamp) and shaped glyph wrapping.
Do not start a row with closing punctuation or end it with opening punctuation,
even when an indivisible pair exceeds the available width. Preserve the character
following an emitted boundary so subsequent punctuation is evaluated correctly.
No invisible characters or manually inserted newlines are added to UI text.

Remove this patch after upstream covers these rules in every wrapping path.
Regression coverage uses real shaped GPUI text at multiple widths in the desktop.
