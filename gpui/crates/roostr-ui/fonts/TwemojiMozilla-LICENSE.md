# TwemojiMozilla.ttf

The browser build's color emoji font: "Twemoji Mozilla" 0.7.0 (Twemoji 14.0),
a COLR/CPAL (COLRv0) build of Twemoji by Mozilla,
<https://github.com/mozilla/twemoji-colr/releases/tag/v0.7.0>
(`Twemoji.Mozilla.ttf`).

## Changes

Made from the release file with fontTools:

- `pyftsubset --unicodes='*' --glyphs='*' --layout-features='*' --no-hinting
  --name-IDs='*' --name-languages='*' --notdef-outline --drop-tables+=FFTM`
  (every glyph, mapping and substitution kept; hinting and the FontForge
  timestamp table dropped), `post` table format 3 (no glyph names).
- PostScript name (name ID 6) set to `NotoColorEmoji`: GPUI's text system
  rasterizes a font's color layers only for that name. The family name is
  unchanged.

## License for the visual design

The emoji art comes from [Twemoji](https://github.com/twitter/twemoji),
Copyright 2019 Twitter, Inc and other contributors, licensed under
[CC-BY 4.0](https://creativecommons.org/licenses/by/4.0/). The changes above
are listed as that license requires.

## License for the font build

Copyright 2016-2018, Mozilla Foundation

Licensed under the Apache License, Version 2.0 (the "License"); you may not
use this file except in compliance with the License. You may obtain a copy of
the License at <http://www.apache.org/licenses/LICENSE-2.0>.

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS, WITHOUT
WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied. See the
License for the specific language governing permissions and limitations under
the License.
