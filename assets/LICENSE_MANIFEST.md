# Assets and their licenses

Every file under `assets/` is either part of Keyglass, under the project's
license (MIT OR Apache-2.0, see `NOTICE`), or a third-party asset listed
below with its license. Third-party files are unmodified copies of the
upstream files named here, except where the table says otherwise.

## Third-party assets

| Files | Project | Upstream source | License |
|---|---|---|---|
| `bundled_fonts/Montserrat-VariableFont_wght.ttf`, `bundled_fonts/Montserrat-Italic-VariableFont_wght.ttf` | Montserrat, by Julieta Ulanovsky and the Montserrat Project Authors | [JulietaUla/Montserrat](https://github.com/JulietaUla/Montserrat) tag `v9.000` (commit `f1f4e9b630b6ac52f1c55c572b29d2a5c3535139`), `fonts/variable/Montserrat[wght].ttf` and `fonts/variable/Montserrat-Italic[wght].ttf`, renamed to the file names Google Fonts uses | SIL Open Font License 1.1, `bundled_fonts/LICENSE` (upstream `OFL.txt`) |
| `weather_icons/*.svg` (18 files) | Weather Icons, by Erik Flowers, original designs by Lukas Bischoff | [erikflowers/weather-icons](https://github.com/erikflowers/weather-icons) release `2.0.12`, `svg/` | SIL Open Font License 1.1, `weather_icons/LICENSE` |
| Lucide icon paths embedded in the display (`keyboard/display/src/assets.rs`) | Lucide, portions derived from Feather | [lucide-icons/lucide](https://github.com/lucide-icons/lucide) release `0.460.0`, `icons/`; a few icons are simplified or filled for the display's small size | ISC, with Feather portions under MIT, `LUCIDE_LICENSE` (upstream `LICENSE`) |

SHA-256 of the bundled files:

```
0f7b311b2f3279e4eef9b2f968bcdbab6e28f4daeb1f049f4f278a902bcd82f7  bundled_fonts/Montserrat-VariableFont_wght.ttf
51607f316bc020e59f03cbf51543eecffbea501c0b31d73e5b82927c5cca442c  bundled_fonts/Montserrat-Italic-VariableFont_wght.ttf
```

The weather icons are `wi-cloudy`, `wi-day-cloudy`, `wi-day-rain-mix`,
`wi-day-sunny`, `wi-fog`, `wi-hail`, `wi-na`, `wi-night-alt-cloudy`,
`wi-night-alt-rain-mix`, `wi-night-clear`, `wi-rain`, `wi-showers`,
`wi-sleet`, `wi-snow-wind`, `wi-snow`, `wi-sprinkle`, `wi-storm-showers` and
`wi-thunderstorm`.

## Keyglass assets

- `default_config.json`: the default appearance, settings, Mosaic layout and
  actuation modes used by the daemon.
- `icons/keyglass.svg`: the Keyglass logo.
- `default_backgrounds/*.png` and `default_backgrounds/thumbnails/*.jpg`: the
  default background images offered as theme presets.

The procedural backgrounds (`aurora_drift`, `cobalt_waves`, `horizon_split`,
`tidal_caustics`, `perimeter_glow`) are part of the display's source code.

## Adding an asset

Add third-party files only with a license that allows redistribution in
binary releases, keep the upstream license text next to the files, and add a
row to the table above naming the exact upstream release or commit. List the
component in the root `NOTICE` as well.
