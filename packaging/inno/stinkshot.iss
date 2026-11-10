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
; PrintScreen value and the cache folder get the same "-<profile>" suffix. The PrintScreen
; setting itself is one Windows value shared by every profile, so a dev profile restores it
; only when the release instance holds no takeover (MayRestoreGlobal).
;
; /DTestBuild=1 builds the smoke-test installer: its own AppId, name, folder and Start menu
; shortcut, so installing and uninstalling it never touches a real install's Apps entry or
; shortcut. Never publish it.

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

#ifdef TestBuild
  #define AppTitle "Stinkshot Test"
  #define SetupBase "Stinkshot-Test"
#else
  #define AppTitle "Stinkshot"
  #define SetupBase "Stinkshot"
#endif

[Setup]
; Never change AppId: winget and upgrades find the installed copy by it ("{AppId}_is1").
#ifdef TestBuild
AppId={{8638BB59-F403-4458-8CD8-7B748885E811}
#else
AppId={{7F697D6C-4CDA-4A4F-8EEB-DA1D24AD69C4}
#endif
AppName={#AppTitle}
AppVersion={#AppVersion}
AppVerName={#AppTitle} {#AppVersion}
AppPublisher=Stinkshot contributors
AppPublisherURL=https://code.mccullough.dev/christian/stinkshot
AppSupportURL=https://code.mccullough.dev/christian/stinkshot/issues
AppUpdatesURL=https://code.mccullough.dev/christian/stinkshot/releases
AppCopyright=Copyright (C) 2026 Stinkshot contributors. GPL-3.0-or-later.
VersionInfoVersion={#NumericVersion}
VersionInfoProductVersion={#NumericVersion}
VersionInfoProductTextVersion={#AppVersion}
VersionInfoDescription=Stinkshot Setup
PrivilegesRequired=lowest
DefaultDirName={autopf}\{#AppTitle}
DisableDirPage=auto
DisableProgramGroupPage=yes
; The Ready page shows only for an update ("Update" button); a fresh install skips it
; (ShouldSkipPage).
DisableReadyPage=no
DisableWelcomePage=yes
UninstallDisplayName={#AppTitle}
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
OutputBaseFilename={#SetupBase}-{#AppVersion}-{#Arch}-setup
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
en.UpdateCaption=Update {#AppTitle}
en.UpdateReadyHeading=Ready to Update
en.UpdateReadyText=Stinkshot %1 will be updated to {#AppVersion}. Your settings, hotkeys and History are kept.
en.UpdateButton=&Update
en.UpdateInstalling=Updating Stinkshot to {#AppVersion}...
en.UpdateDoneHeading=Stinkshot Is Updated
en.UpdateDoneText=Stinkshot was updated from %1 to {#AppVersion}. Your settings, hotkeys and History are kept.

[Tasks]
Name: "autostart"; Description: "{cm:AutostartTask}"; Flags: unchecked

[Files]
Source: "{#StageDir}\stinkshot.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\LICENSE.txt"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppTitle}"; Filename: "{app}\stinkshot.exe"; Comment: "Screenshots and screen recording"

[Registry]
; Same value the app's own Start with Windows setting writes, so either one can turn it off.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "{code:RunValueName}"; ValueData: """{app}\stinkshot.exe"" --tray"; Tasks: autostart

[Run]
Filename: "{app}\stinkshot.exe"; Description: "{cm:LaunchNow}"; Flags: nowait postinstall skipifsilent
; A silent upgrade (winget upgrade) that closed a running copy starts it again in the tray, so
; the tray icon and the PrintScreen takeover come back.
Filename: "{app}\stinkshot.exe"; Parameters: "--tray"; Flags: nowait runasoriginaluser; Check: RelaunchAfterSilentUpgrade

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

var
  WasRunning: Boolean;
  PrevVersion: String;

{ The version already installed under this AppId, or '' for a fresh install. }
function InstalledVersion(): String;
var
  Key: String;
begin
  Result := '';
  Key := 'Software\Microsoft\Windows\CurrentVersion\Uninstall\' +
    ExpandConstant('{#emit SetupSetting("AppId")}') + '_is1';
  if not RegQueryStringValue(HKCU, Key, 'DisplayVersion', Result) then
    Result := '';
end;

function IsUpdate(): Boolean;
begin
  Result := PrevVersion <> '';
end;

function InitializeSetup(): Boolean;
begin
  PrevVersion := InstalledVersion();
  Log(Format('installed version before setup: "%s"', [PrevVersion]));
  Result := True;
end;

procedure InitializeWizard();
begin
  if IsUpdate() then
    WizardForm.Caption := CustomMessage('UpdateCaption');
end;

{ An update keeps the previous folder and tasks, so it skips straight to Ready; a fresh install
  never shows Ready. }
function ShouldSkipPage(PageID: Integer): Boolean;
begin
  Result := False;
  if IsUpdate() then
    Result := (PageID = wpSelectDir) or (PageID = wpSelectTasks)
  else
    Result := PageID = wpReady;
end;

procedure CurPageChanged(CurPageID: Integer);
begin
  if IsUpdate() then
  begin
    if CurPageID = wpReady then
    begin
      WizardForm.PageNameLabel.Caption := CustomMessage('UpdateReadyHeading');
      WizardForm.PageDescriptionLabel.Caption := '';
      WizardForm.ReadyLabel.Caption := FmtMessage(CustomMessage('UpdateReadyText'), [PrevVersion]);
      WizardForm.ReadyMemo.Visible := False;
      WizardForm.NextButton.Caption := CustomMessage('UpdateButton');
    end
    else if CurPageID = wpInstalling then
      WizardForm.PageNameLabel.Caption := CustomMessage('UpdateInstalling')
    else if CurPageID = wpFinished then
    begin
      WizardForm.FinishedHeadingLabel.Caption := CustomMessage('UpdateDoneHeading');
      WizardForm.FinishedLabel.Caption := FmtMessage(CustomMessage('UpdateDoneText'), [PrevVersion]);
    end;
  end
  else if CurPageID = wpSelectTasks then
    { Ready is skipped, so Tasks is the last page before installing. }
    WizardForm.NextButton.Caption := SetupMessage(msgButtonInstall);
end;

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

{ Ask a running copy of this install to exit before its exe is replaced, remembering whether
  one was running (its single-instance mutex, see platform::system::acquire_single_instance). }
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Exe: String;
  Code: Integer;
begin
  Result := '';
  Exe := ExpandConstant('{app}\stinkshot.exe');
  WasRunning := False;
  if FileExists(Exe) then
  begin
    WasRunning := CheckForMutexes('Local\StinkShot.Instance' + ProfileSuffix());
    Log(Format('running instance before upgrade: %d', [Ord(WasRunning)]));
    if Exec(Exe, 'quit', '', SW_HIDE, ewWaitUntilTerminated, Code) then
      Log(Format('stinkshot quit exited with %d', [Code]))
    else
      Log('stinkshot quit could not be started');
  end;
end;

function RelaunchAfterSilentUpgrade(): Boolean;
begin
  Result := WasRunning and WizardSilent();
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

{ Mirrors platform::system::may_restore_global: the Windows value is shared by every profile.
  The release install always restores it; a dev profile only when the release instance holds
  no takeover and the value is still the 0 a takeover set. }
function MayRestoreGlobal(): Boolean;
var
  Current: Cardinal;
begin
  if ProfileSuffix() = '' then
    Result := True
  else if RegValueExists(HKCU, AppKey, 'PrevPrintScreenSnipping') then
    Result := False
  else
    Result := RegQueryDWordValue(HKCU, KeyboardKey, PrtScValue, Current) and (Current = 0);
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
    if not MayRestoreGlobal() then
      Log('PrintScreen left as is: another Stinkshot profile holds it')
    else if Prev = PrevAbsent then
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
