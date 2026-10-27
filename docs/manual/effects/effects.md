# Built-in effects

Oxide comes with its own effects, written for it and checked against reference renders. They
work anywhere a plug-in does: on a track, a folder, a bus, the master, or the Waveform view's effects rack.
They cost little CPU, add no delay unless they need to look ahead, and play the same at any
buffer size, live and in an export.

## Adding one

1. Click **+** in a track's inserts (the mixer strip or the inspector), an empty slot in the
   header's rack, or one in the Waveform view's rack.
2. The plug-in browser lists the built-in effects first, under **Built in**. Type part of a
   name (*comp*, *reverb*) to find one.
3. Click it. It goes into the chain like any plug-in ([[page:mixer#inserts-adding-plug-ins]]).

## The editor

Click the effect's name (on the strip, in the inspector or in the rack) to open its editor, a
small window titled with the effect and where it is (*Compressor · Guitar L*).
Click the name again, or the window's ✕, to close it.

::: shot effect-compressor
crop effect.editor 6
highlight effect.curve "1" "What the settings do: here the compressor's curve, input level across, output up."
highlight effect.params "2" "The settings: drag a slider, click its number to type, double-click it for the default."
The Compressor's editor, on the Guitars folder.
:::

- **Sliders** set each parameter, in its own units (dB, Hz, ms, %). Double-click a slider to
  put it back to its default; click the number to type a value.
- **Switches** are boxes (on or off); a few are a pair of named buttons (the Phaser's
  *Triangle* / *Sine*).
- **Pictures** above the settings show what they do, and move as you change them: the
  equalisers' and the Hum Remover's frequency response, the dynamics' input-to-output curve, the
  delays' taps and repeats, the reverbs' decay, a noise print.
- Effects with many settings group them in sections that fold (**Reverberation**,
  **Coloration**, …) and scroll.

Every change is part of the project: saved with it, and undone with [[Ctrl+Z]]. F1 over an
editor opens that effect's section of this manual.

The editors don't offer presets of their own yet; set the parameters by hand, or save a whole
rack of effects as a preset in the Waveform view ([[page:waveform-view#rack-presets]]).

## Automation and delay

Most parameters can have an automation lane on their track ([[page:automation]]), moving
sample-accurately. The few that can't (they change the effect's delay, or rebuild what it
plays) are marked *No automation lane* in each effect's table below.

Some effects need to look ahead (a limiter, the linear-phase Graphic EQ, the noise
reducers): each table gives the delay at 48 kHz. Oxide delays every other track to match, so
everything stays in time; the status bar's PDC shows the total
([[page:choppy-audio#the-status-bar]]).

## The effects

The effects come in the plug-in browser's groups:

| group | effects |
|---|---|
| [[page:effects-dynamics]] | [[effect:amplify]], [[effect:channel-mixer]], [[effect:limiter]], [[effect:compressor]], [[effect:de-esser]], [[effect:dynamics]], [[effect:multiband-compressor]] |
| [[page:effects-eq]] | [[effect:parametric-eq]], the Graphic EQ with [[effect:graphic-eq-10|10]], [[effect:graphic-eq-20|20]] or [[effect:graphic-eq-30|30]] bands |
| [[page:effects-restoration]] | [[effect:noise-reduction]], [[effect:hiss-reduction]], [[effect:auto-noise-reduction]], [[effect:hum-remover]], [[effect:click-remover]], [[effect:click-pop-repair]], [[effect:clip-repair]] |
| [[page:effects-reverb]] | [[effect:room-reverb]], [[effect:convolution-reverb]], [[effect:space-reverb]] |
| [[page:effects-delay]] | [[effect:delay]], [[effect:echo]], [[effect:vintage-delay]] |
| [[page:effects-modulation]] | [[effect:chorus]], [[effect:flanger]], [[effect:phaser]] |

Each effect's section says what it does and when to reach for it, shows its editor, and
lists every parameter with its range and default. The tables are made from the effects
themselves when the manual is built, so they always match the app.

## Over MCP

[[action:add_insert]] adds a built-in effect by its name (*Room Reverb*);
[[action:list_plugin_parameters]] and [[action:set_plugin_parameter]] read and set its
parameters; [[action:set_impulse_file]] gives the Convolution Reverb its impulse;
[[action:capture_noise_print]] teaches a noise reducer, Clip Repair or Click & Pop
Repair.

## See also

- [[page:mixer]]: inserts, bypass and plug-in windows.
- [[page:waveform-view#the-effects-rack]]: running effects into a file.
