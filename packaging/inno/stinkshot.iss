; Stinkshot per-user installer (Inno Setup 7). Built by scripts/release.ps1, which passes:
;   /DAppVersion=0.1.0  /DArch=x64|arm64  /DStageDir=<folder with stinkshot.exe, LICENSE.txt>
;   /DOutputDir=<dist folder>  [/DIconFile=<stinkshot.ico>]  [/DSignTool=1 with /Ssigntool=...]
;
; Per-user only: installs to %LOCALAPPDATA%\Programs\Stinkshot, never asks for admin.
; Silent: Setup.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART [/TASKS="autostart"]
; Uninstall: unins000.exe /VERYSILENT  (also from Settings > Apps, and winget uninstall)
;
; Uninstall removes only what this installer put down, the start-with-Windows entry when it
; points at this copy, the Snipping Tool PrintScreen value Stinkshot replaced (restored), and
; the logo cache. Captures, history and config.toml are never touched.
;
; STINKSHOT_PROFILE (dev/test only) is honored like the app does: the Run value, the saved
; PrintScreen value and the cache folder get the same "-<profile>" suffix, so a test install
; never touches the real instance's state.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef Arch
  #define Arch "x64"
#endif
#ifndef StageDir
  #define StageDir "..\..\dist\stage-" + Arch
#endif
#ifndef OutputDir
  #define OutputDir "..\..\dist"
#endif

; Numeric x.y.z for VERSIONINFO: drop any "-beta.1" suffix.
#define NumericVersion Copy(AppVersion, 1, Pos("-", AppVersion + "-") - 1)

[Setup]
; Never change AppId: winget and upgrades find the installed copy by it ("{AppId}_is1").
AppId={{7F697D6C-4CDA-4A4F-8EEB-DA1D24AD69C4}
AppName=Stinkshot
AppVersion={#AppVersion}
AppVerName=Stinkshot {#AppVersion}
AppPublisher=Stinkshot contributors
AppPublisherURL=https://code.mccullough.dev/beard/stinkshot
AppSupportURL=https://code.mccullough.dev/beard/stinkshot/issues
AppUpdatesURL=https://code.mccullough.dev/beard/stinkshot/releases
AppCopyright=Copyright (C) 2026 Stinkshot contributors. GPL-3.0-or-later.
VersionInfoVersion={#NumericVersion}
VersionInfoProductVersion={#NumericVersion}
VersionInfoProductTextVersion={#AppVersion}
VersionInfoDescription=Stinkshot Setup
PrivilegesRequired=lowest
DefaultDirName={autopf}\Stinkshot
DisableDirPage=auto
DisableProgramGroupPage=yes
DisableReadyPage=yes
DisableWelcomePage=yes
UninstallDisplayName=Stinkshot
UninstallDisplayIcon={app}\stinkshot.exe
#if Arch == "arm64"
ArchitecturesAllowed=arm64
ArchitecturesInstallIn64BitMode=arm64
#else
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
#endif
; Windows 10 1903: Windows Graphics Capture.
MinVersion=10.0.18362
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
; The tray app holds stinkshot.exe open: PrepareToInstall asks it to quit gracefully first
; (`stinkshot quit` waits up to 35 s for saves and recordings); Restart Manager then force-closes
; anything left, since the tray app has no window that answers WM_CLOSE.
CloseApplications=force
RestartApplications=no
OutputDir={#OutputDir}
OutputBaseFilename=Stinkshot-{#AppVersion}-{#Arch}-setup
#ifdef IconFile
SetupIconFile={#IconFile}
#endif
#ifdef SignTool
SignTool=signtool
SignedUninstaller=yes
#endif

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"

[CustomMessages]
en.AutostartTask=Start Stinkshot when I sign in
en.LaunchNow=Launch Stinkshot

[Tasks]
Name: "autostart"; Description: "{cm:AutostartTask}"; Flags: unchecked

[Files]
Source: "{#StageDir}\stinkshot.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\LICENSE.txt"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Stinkshot"; Filename: "{app}\stinkshot.exe"; Comment: "Screenshots and screen recording"

[Registry]
; Same value the app's own Start with Windows setting writes, so either one can turn it off.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "{code:RunValueName}"; ValueData: """{app}\stinkshot.exe"" --tray"; Tasks: autostart

[Run]
Filename: "{app}\stinkshot.exe"; Description: "{cm:LaunchNow}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; Graceful exit: finishes saves and recordings and restores PrintScreen like Quit in the tray.
Filename: "{app}\stinkshot.exe"; Parameters: "quit"; Flags: runhidden waituntilterminated; RunOnceId: "QuitStinkshot"

[Code]
const
  RunKey = 'Software\Microsoft\Windows\CurrentVersion\Run';
  AppKey = 'Software\StinkShot';
  KeyboardKey = 'Control Panel\Keyboard';
  PrtScValue = 'PrintScreenKeyForSnippingEnabled';
  PrevAbsent = $FFFFFFFF;

{ Mirrors stinkshot_core::config::profile_suffix: "" normally, "-<name>" for a dev profile. }
function ProfileSuffix(): String;
var
  Raw, S: String;
  I: Integer;
  C: Char;
begin
  Raw := GetEnv('STINKSHOT_PROFILE');
  S := '';
  for I := 1 to Length(Raw) do
  begin
    C := Raw[I];
    if ((C >= 'a') and (C <= 'z')) or ((C >= 'A') and (C <= 'Z')) or
       ((C >= '0') and (C <= '9')) or (C = '-') or (C = '_') then
      if Length(S) < 32 then
        S := S + C;
  end;
  if S = '' then
    Result := ''
  else
    Result := '-' + S;
end;

function RunValueName(Param: String): String;
begin
  Result := 'StinkShot' + ProfileSuffix();
end;

{ Ask a running copy of this install to exit before its exe is replaced. }
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Exe: String;
  Code: Integer;
begin
  Result := '';
  Exe := ExpandConstant('{app}\stinkshot.exe');
  if FileExists(Exe) then
  begin
    if Exec(Exe, 'quit', '', SW_HIDE, ewWaitUntilTerminated, Code) then
      Log(Format('stinkshot quit exited with %d', [Code]))
    else
      Log('stinkshot quit could not be started');
  end;
end;

procedure RemoveAutostartIfOurs();
var
  Name, Value, Exe: String;
begin
  Name := RunValueName('');
  Exe := Lowercase(ExpandConstant('{app}\stinkshot.exe'));
  if RegQueryStringValue(HKCU, RunKey, Name, Value) then
    if Pos(Exe, Lowercase(Value)) > 0 then
      RegDeleteValue(HKCU, RunKey, Name);
end;

{ The app restores this itself when it quits; this covers a copy that was killed. }
procedure RestorePrintScreen();
var
  Name: String;
  Prev: Cardinal;
begin
  Name := 'PrevPrintScreenSnipping' + ProfileSuffix();
  if RegQueryDWordValue(HKCU, AppKey, Name, Prev) then
  begin
    if Prev = PrevAbsent then
      RegDeleteValue(HKCU, KeyboardKey, PrtScValue)
    else
      RegWriteDWordValue(HKCU, KeyboardKey, PrtScValue, Prev);
    RegDeleteValue(HKCU, AppKey, Name);
  end;
  RegDeleteKeyIfEmpty(HKCU, AppKey);
end;

procedure RemoveCache();
var
  Dir: String;
begin
  Dir := ExpandConstant('{localappdata}\Stinkshot') + ProfileSuffix();
  DelTree(Dir + '\cache', True, True, True);
  RemoveDir(Dir);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
  begin
    RemoveAutostartIfOurs();
    RestorePrintScreen();
    RemoveCache();
  end;
end;
