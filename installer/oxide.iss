; Oxide's Windows installer (Inno Setup 7; dev plan 19.1). Built by scripts/installer.ps1, which
; stages the files and passes the defines below; compile it through that script, not by hand.
;
; - Per user, no administrator: PrivilegesRequired=lowest. The program keeps its own files beside
;   itself (settings.json, rack.json, Templates\, RackPresets\, mcp-bridge\), so its folder must
;   be the user's to write: G:\Oxide by default, never Program Files.
; - Updates in place: the same AppId finds the last install's folder (UsePreviousAppDir) and
;   skips the folder page (DisableDirPage=auto). Only the files listed in [Files] are installed
;   or replaced; the user's files above are never installed, so an update can't touch them.
;   manual\ (the HTML manual, 27.3) is the installer's: removed whole before an update installs
;   the new one, and on uninstall.
; - A downgrade is refused unless Setup is run with /ALLOWDOWNGRADE.
; - Oxide holds the mutex "OxideDAW" while it runs (daw-app's main.rs, RUNNING_MUTEX): Setup and
;   the uninstaller ask for it to be closed first, so a running exe is never overwritten and
;   unsaved work is the user's to save.
; - The uninstaller asks whether to keep the user's files (default: keep). Silently it keeps
;   them, unless run with /DELETEUSERFILES.
; - The .oxide association is the Settings button's (daw-app's settings.rs, file_type_entries;
;   its test `the_installer_registers_oxide_files_as_the_settings_button_does` reads [Registry]).
;
; Check builds (installer.ps1 -Check, for the maintainer's install check) have their own AppId and name,
; so they never meet the real installation, and make no shortcuts, association or mutex check.

#ifndef AppVersion
  #error AppVersion is not defined: build with scripts/installer.ps1
#endif
#ifndef AppCommit
  #define AppCommit "unknown"
#endif
#ifndef Stage
  #error Stage (the staged files' folder) is not defined: build with scripts/installer.ps1
#endif

#ifdef CheckBuild
  ; Only ever used by the installer check, in out\check\install.
  #define AppGuid "A62AD54F-4E8A-4C7E-A8DA-1A9D2B0D45EE"
  #define AppName "Oxide Installer Check"
  #define SetupName "OxideCheck-Setup-" + AppVersion
#else
  #define AppGuid "89005345-A6F4-49A5-8AD8-EACC60997356"
  #define AppName "Oxide"
  #define SetupName "Oxide-Setup-" + AppVersion
#endif
#define AppMutex "OxideDAW"
#define UninstallKey "Software\Microsoft\Windows\CurrentVersion\Uninstall\{" + AppGuid + "}_is1"

[Setup]
AppId={{{#AppGuid}}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=Oxide
AppComments=Built from commit {#AppCommit}
VersionInfoVersion={#AppVersion}
; The commit is in AppComments (the uninstall entry): Windows cuts the file's version text short.
VersionInfoTextVersion={#AppVersion}
VersionInfoProductName=Oxide
VersionInfoDescription={#AppName} Setup
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
#ifdef DefaultDir
DefaultDirName={#DefaultDir}
#else
DefaultDirName={code:DefaultDir}
#endif
DisableDirPage=auto
UsePreviousAppDir=yes
DisableProgramGroupPage=yes
DisableWelcomePage=yes
#ifndef CheckBuild
AppMutex={#AppMutex}
ChangesAssociations=yes
#endif
; Files in use by a program without the mutex (an older Oxide): Restart Manager asks to close it.
; Never restarted afterwards: Setup's own [Run] entry offers to start Oxide.
CloseApplications=yes
RestartApplications=no
SetupIconFile={#Stage}\oxide.ico
UninstallDisplayIcon={app}\Oxide.exe
UninstallDisplayName={#AppName}
OutputBaseFilename={#SetupName}
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes

#ifndef CheckBuild
[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; Flags: unchecked
Name: "fileassoc"; Description: "&Open .oxide projects with Oxide (for your account only)"
#endif

[Files]
Source: "{#Stage}\Oxide.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\oxide-mcp.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\oxide.ico"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\README.txt"; DestDir: "{app}"; Flags: ignoreversion
; Oxide's licence (GPLv3) and its notice (the section 7(e) term on the name and logo), as
; .txt so they open in Notepad.
Source: "{#Stage}\LICENSE.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\NOTICE.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Stage}\THIRD-PARTY.txt"; DestDir: "{app}"; Flags: ignoreversion
; The manual (27.3), built by installer.ps1: Help > Manual and F1 open manual\index.html.
Source: "{#Stage}\manual\*"; DestDir: "{app}\manual"; Flags: ignoreversion recursesubdirs createallsubdirs

; An update replaces the whole manual: pages and pictures an older version had and this one
; doesn't would otherwise stay (and be found by its search). manual\ is the installer's own,
; never the user's: their files are beside it ([Code]'s UserFiles).
[InstallDelete]
Type: filesandordirs; Name: "{app}\manual"

; Uninstalling removes it whole, whatever was added to it since.
[UninstallDelete]
Type: filesandordirs; Name: "{app}\manual"

#ifndef CheckBuild
[Icons]
Name: "{autoprograms}\Oxide"; Filename: "{app}\Oxide.exe"
Name: "{autodesktop}\Oxide"; Filename: "{app}\Oxide.exe"; Tasks: desktopicon

; The same keys and values as Settings > Projects > "Open .oxide files with Oxide".
[Registry]
Root: HKCU; Subkey: "Software\Classes\.oxide"; ValueType: string; ValueName: ""; ValueData: "Oxide.Project"; Tasks: fileassoc
Root: HKCU; Subkey: "Software\Classes\Oxide.Project"; ValueType: string; ValueName: ""; ValueData: "Oxide project"; Tasks: fileassoc
Root: HKCU; Subkey: "Software\Classes\Oxide.Project\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: """{app}\Oxide.exe"",0"; Tasks: fileassoc
Root: HKCU; Subkey: "Software\Classes\Oxide.Project\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\Oxide.exe"" ""%1"""; Tasks: fileassoc

[Run]
Filename: "{app}\Oxide.exe"; Description: "Start Oxide"; Flags: nowait postinstall skipifsilent
#endif

[Code]
const
  UninstallKey = '{#UninstallKey}';

var
  KeepUserFiles: Boolean;

{ Whether Setup or the uninstaller was run with the switch Name (/ALLOWDOWNGRADE). }
function HasSwitch(const Name: String): Boolean;
var
  I: Integer;
begin
  Result := False;
  for I := 1 to ParamCount do
    if CompareText(ParamStr(I), Name) = 0 then
      Result := True;
end;

{ G:\Oxide where there is a G: drive (the user's), else the user's own programs folder. }
function DefaultDir(Param: String): String;
begin
  if DirExists('G:\') then
    Result := 'G:\Oxide'
  else
    Result := ExpandConstant('{userpf}\Oxide');
end;

{ Refuse to install over a newer version, unless asked to with /ALLOWDOWNGRADE. }
function InitializeSetup(): Boolean;
var
  Installed: String;
  OldVersion, NewVersion: Int64;
begin
  Result := True;
  if RegQueryStringValue(HKCU, UninstallKey, 'DisplayVersion', Installed)
    and StrToVersion(Installed, OldVersion) and StrToVersion('{#AppVersion}', NewVersion)
    and (ComparePackedVersion(OldVersion, NewVersion) > 0) and not HasSwitch('/ALLOWDOWNGRADE') then
  begin
    SuppressibleMsgBox('{#AppName} ' + Installed + ' is installed, which is newer than this ' +
      'installer''s {#AppVersion}. Nothing was changed.' + #13#10#13#10 +
      'To go back to {#AppVersion} anyway, run this installer with /ALLOWDOWNGRADE.',
      mbError, MB_OK, IDOK);
    Result := False;
  end;
end;

{ The program's own files beside it: never installed, so updates leave them alone. }
function UserFiles(): TArrayOfString;
begin
  SetArrayLength(Result, 7);
  Result[0] := 'settings.json';
  Result[1] := 'rack.json';
  Result[2] := 'rack.json.writing';
  Result[3] := 'rack.json.unreadable';
  Result[4] := 'Templates';
  Result[5] := 'RackPresets';
  Result[6] := 'mcp-bridge';
end;

function HasUserFiles(): Boolean;
var
  Files: TArrayOfString;
  I: Integer;
begin
  Result := False;
  Files := UserFiles();
  for I := 0 to GetArrayLength(Files) - 1 do
    if FileExists(ExpandConstant('{app}\') + Files[I]) or DirExists(ExpandConstant('{app}\') + Files[I]) then
      Result := True;
end;

procedure DeleteUserFiles();
var
  Files: TArrayOfString;
  I: Integer;
  Path: String;
begin
  Files := UserFiles();
  for I := 0 to GetArrayLength(Files) - 1 do
  begin
    Path := ExpandConstant('{app}\') + Files[I];
    if DirExists(Path) then
      DelTree(Path, True, True, True)
    else if FileExists(Path) then
      DeleteFile(Path);
  end;
end;

{ The .oxide registration (the installer's or the Settings button's) goes with the program it
  names: only when it still opens this folder's Oxide.exe. }
procedure RemoveAssociation();
var
  Command, Extension: String;
begin
  if RegQueryStringValue(HKCU, 'Software\Classes\Oxide.Project\shell\open\command', '', Command)
    and (CompareText(Command, '"' + ExpandConstant('{app}') + '\Oxide.exe" "%1"') = 0) then
  begin
    RegDeleteKeyIncludingSubkeys(HKCU, 'Software\Classes\Oxide.Project');
    if RegQueryStringValue(HKCU, 'Software\Classes\.oxide', '', Extension)
      and (Extension = 'Oxide.Project') then
      RegDeleteKeyIncludingSubkeys(HKCU, 'Software\Classes\.oxide');
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
  begin
    KeepUserFiles := True;
    if UninstallSilent() then
      KeepUserFiles := not HasSwitch('/DELETEUSERFILES')
    else if HasUserFiles() then
      { No line may start with #: the preprocessor would take it for a directive. }
      KeepUserFiles := MsgBox('Keep your Oxide settings, effects rack, templates and rack ' +
        'presets?' + #13#10#13#10 + 'They are in ' + ExpandConstant('{app}') + '. Keep them to find them ' +
        'again if you install Oxide here later; your projects are elsewhere and stay either way.',
        mbConfirmation, MB_YESNO) = IDYES;
  end;
  if CurUninstallStep = usPostUninstall then
  begin
    RemoveAssociation();
    if not KeepUserFiles then
    begin
      DeleteUserFiles();
      { The folder itself only if nothing else is left in it. }
      RemoveDir(ExpandConstant('{app}'));
    end;
  end;
end;
