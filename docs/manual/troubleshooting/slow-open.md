# A project opens slowly

Opening a project with many plug-ins takes a while: every plug-in has to be created and given
its settings, and a heavy amp sim or mastering suite can take a second or more on its own.
Oxide shows a loading bar meanwhile and stays responsive.

::: shot busy
crop 527 390 386 120
The loading bar while a project opens.
:::

## What the loading bar says

The bar's title is *Opening* and the project's name. The line under it says what Oxide is on
now, in three stages:

1. **Closing** the plug-ins of the project you had open (*Closing Amp Sim (1 of 3)*).
2. **Reading** the audio files (*Reading Guitar.wav (3 of 12)*). Files are read side by
   side in the background.
3. **Loading** each plug-in, one at a time, with the track it is on (*Loading Hard
   Limiter on Guitar L (3 of 6)*).

When the bar names a plug-in for a long time, that plug-in is the slow one: plug-ins must be
created one by one, on the same thread as the window.

Until everything is in, the window shows only the bar. **Cancel** stops the opening and goes
back to the project you had open.

## Other bars

The same bar appears when:

- an **undo or redo** brings plug-ins back (*Undoing Remove insert*): the mix you had plays
  until they are in;
- the **sample rate** changes (*Changing the sample rate to 44.1 kHz*): every plug-in is loaded
  again at the new rate;
- the Waveform view's **effects rack** comes back when Oxide starts (*Loading the effects
  rack*);
- you **save** (*Saving*, then *Reading* each plug-in's settings and *Writing* the file);
  before closing a project with changes, Oxide reads them the same way (*Looking for changes*).

## Making it faster

- A project that was made at another sample rate is converted as it opens: keep projects at
  the device's rate.
- Remove plug-ins a project no longer uses, rather than bypassing them: a bypassed plug-in
  still loads.
- Render a finished track with heavy plug-ins to audio (Track > **Render track…**): its render
  plays instead, and its plug-ins can be removed.

## Over MCP

While a project opens, [[action:get_busy]] reports the stage and what it is on, and
[[action:cancel_open]] cancels it; other calls wait until the project is in.

## See also

- [[page:plugin-problems]]: a plug-in that doesn't load at all.
- [[page:getting-started#open-a-project]].
