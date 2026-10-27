Oxide
=====

A digital audio workstation for recording and mastering in one program.

What is in this folder
  Oxide.exe         the program
  oxide-mcp.exe     the bridge Claude Desktop runs to reach Oxide (Settings > AI control)
  oxide.ico         the icon
  LICENSE.txt       Oxide's licence, the GNU General Public License version 3: use,
                    change and share Oxide under its terms; the music you make is yours
  NOTICE.txt        the copyright notice and the licence's one added term: it doesn't
                    cover the name "Oxide" or its logo
  THIRD-PARTY.txt   the libraries Oxide includes and their licences
  manual\           the manual (Help > Manual, or F1 for the page about where you are);
                    open manual\index.html in any web browser
  unins000.exe      the uninstaller (also in Settings > Apps > Installed apps)

Oxide also keeps its own files here, beside the program, as you use it:
  settings.json     your settings
  rack.json         the Waveform view's effects rack
  Templates\        project templates
  RackPresets\      effects rack presets
  mcp-bridge\       the copy of the bridge Claude Desktop runs

Your projects are wherever you saved them, never in this folder unless you put them here.

Updating
  Run a newer Oxide-Setup-<version>.exe. It finds this folder and replaces the program only;
  the files Oxide keeps here are left as they are. Close Oxide first (the installer asks).
  An older installer refuses to replace a newer Oxide (run it with /ALLOWDOWNGRADE to go back).

Uninstalling
  Settings > Apps > Installed apps > Oxide > Uninstall, or unins000.exe here. It asks whether
  to keep your settings, effects rack, templates and rack presets (the default is to keep them).
