# ICE — Clarity in motion

## Idea

Turn an open-ended intention into clear, visible progress. The identity combines
precise terminal typography with a curious ice companion, **FLOE**.

Product name: **ICE**. Expansion: **Intent · Compile · Execute**.
Product promise: **A clear path from thought to shipped.**
Primary headline: **Clarity in motion.**

## Visual system

| Token | Color | Role |
| --- | --- | --- |
| Midnight | `#061018` | Primary canvas |
| Deep water | `#0A1C28` | Raised surfaces |
| Glacier | `#50D2FF` | Focus and primary actions |
| Frost | `#A0ECFF` | Logo and cube top |
| Snow | `#E2F4FC` | Body text and highlights |
| Blue slate | `#6E9CB0` | Secondary labels |
| Ice shadow | `#2078A0` | Cube side |
| Mint | `#40DCAA` | Success |
| Amber | `#F0C450` | Running / attention |
| Coral | `#FF6078` | Errors |

Use the user's terminal monospace font. Consolas is the review-artifact font.
No Nerd Font is required. Keep logos in their original proportions. Leave at
least one eye-width around the mascot, and one letter-width around the lockup.
Prefer the full logo above 160 px; use the mascot alone for icons. SVG wordmarks
use live monospace text, so exact letterforms depend on the installed font.

## FLOE and motion

FLOE is an isometric pixel ice cube: bright top, cyan front, blue side,
two square eyes and a small smile. Small ice fragments echo the same geometry.
The TUI uses character cells and shaded faces to suggest 3D; it does not require
a GPU or a terminal graphics protocol.

Motion runs at approximately 12.5 updates per second. Bobbing and glances are
slow; a short blink appears about every five seconds. Busy state adds a rotating
status mark. Set `ICE_REDUCED_MOTION=1` to freeze animation. Motion must never
move the input, controls or conversation layout.

## Voice

Calm, direct and useful. Name the next action. Report actual execution results.
Use short headings and plain language. Avoid promises of autonomous success or
unverified performance claims. Demo and live modes must always be distinguishable.

## Assets

- `logo.svg`: primary editable lockup for dark surfaces.
- `logo-mono.svg`: light monochrome lockup for dark surfaces.
- `floe.svg`, `floe.png`: companion mark with transparency.
- `ice.ico`: Windows application/shortcut icon, 16–256 px.
- `identity-board.png`: identity overview.
- `preview.html`: local animated UI and identity review.
- `welcome.png`, `connect.png`, `session.png`: actual Ratatui buffer captures.
- `ice-preview.gif`: animated welcome capture.

The session capture contains illustrative messages, not a recorded live model run.
