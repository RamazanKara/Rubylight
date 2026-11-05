; The Nightjar for Windows installer (Inno Setup 6). build-installer.ps1 compiles it from the
; unpacked release zip, so the installer carries exactly the zip's files:
;
;   ISCC.exe /DAppVersion=4.0.0 /DNumericVersion=4.0.0.0 /DSourceDir=<unpacked zip>
;            /DOutputDir=<folder> /DOutputBaseName=<name> nightjar.iss
;
; Per user, no administrator prompt: %LOCALAPPDATA%\Programs\Nightjar, a Start menu shortcut, an
; optional desktop shortcut and an Apps & features entry. The client keeps everything in
; %APPDATA%\Nightjar\config (config.rs, config_dir), outside the install folder: an upgrade never
; touches it, and an uninstall removes it only when the user ticks the box for it (or passes
; /REMOVESETTINGS to a silent uninstall), and even then keeps the screenshots and the saved
; statistics, which are the user's own files.
;
; The client's Update now runs this Setup with /VERYSILENT and /RELAUNCH: same folder and tasks as
; the install it upgrades (Inno Setup's previous-install defaults), Nightjar started again at the end.
;
; /DTestDataDir=<folder> builds a test installer that uses <folder> in place of
; %APPDATA%\Nightjar, so build-installer.ps1 -Smoke can check the removal on a developer's PC
; without touching the real settings. Release builds never define it.

#ifndef AppVersion
  #error Pass /DAppVersion=<version>
#endif
#ifndef NumericVersion
  #error Pass /DNumericVersion=<a.b.c.d>
#endif
#ifndef SourceDir
  #error Pass /DSourceDir=<the unpacked zip>
#endif
#ifndef OutputDir
  #define OutputDir "."
#endif
#ifndef OutputBaseName
  #define OutputBaseName "nightjar-desktop-" + AppVersion + "-windows-x86_64-setup"
#endif

; The DLLs this Setup ships, as |a.dll|b.dll|, for RemoveStaleDlls.
#define ShippedDlls "|"
#define DllFind
#define DllFound
#sub AddShippedDll
  #expr ShippedDlls = ShippedDlls + LowerCase(FindGetFileName(DllFind)) + "|"
#endsub
#for {DllFind = DllFound = FindFirst(SourceDir + "\*.dll", 0); DllFound; DllFound = FindNext(DllFind)} AddShippedDll
#if DllFind
  #expr FindClose(DllFind)
#endif

[Setup]
; The AppId ties upgrades and the uninstall entry together: never change it.
AppId={{6A3C8F12-5E7B-4D2A-9C41-7B0E8D3F2A65}
AppName=Nightjar
AppVersion={#AppVersion}
AppVerName=Nightjar {#AppVersion}
AppPublisher=RamazanKara
AppPublisherURL=https://github.com/RamazanKara/nightjar
AppSupportURL=https://github.com/RamazanKara/nightjar/issues
AppUpdatesURL=https://github.com/RamazanKara/nightjar/releases
AppCopyright=Copyright (C) 2026 RamazanKara. GPL-3.0-only.
VersionInfoVersion={#NumericVersion}
VersionInfoTextVersion={#AppVersion}
VersionInfoDescription=Nightjar setup
; Per user: {autopf} is %LOCALAPPDATA%\Programs, and Setup never asks for elevation.
PrivilegesRequired=lowest
DefaultDirName={autopf}\Nightjar
DefaultGroupName=Nightjar
DisableDirPage=yes
DisableProgramGroupPage=yes
DisableReadyPage=yes
; English or German by the Windows display language, without asking.
ShowLanguageDialog=no
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
WizardStyle=modern
SetupIconFile=..\icons\nightjar.ico
WizardImageFile=..\icons\wizard.png
WizardSmallImageFile=..\icons\128.png
UninstallDisplayIcon={app}\nightjar.ico
UninstallDisplayName=Nightjar
; A running Nightjar is closed (Restart Manager) before its files are replaced.
CloseApplications=yes
; But never a stream, in any Nightjar process: Setup stops before it deletes anything. The same
; name as STREAM_MUTEX in self_update.rs.
AppMutex=Local\NightjarStream
RestartApplications=no
Compression=lzma2/max
SolidCompression=yes
OutputDir={#OutputDir}
OutputBaseFilename={#OutputBaseName}
; ---- CODE SIGNING HOOK ----
; There is no certificate yet, so the installer and the uninstaller are not signed. With one,
; build-installer.ps1 -SignCommand "<signtool command with $f for the file>" defines Sign and
; hands ISCC the command as the sign tool "nightjarsign"; nightjar.exe and the DLLs are signed
; before (see build-installer.ps1).
#ifdef Sign
SignTool=nightjarsign
SignedUninstaller=yes
#endif

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "de"; MessagesFile: "compiler:Languages\German.isl"

[CustomMessages]
en.SettingsKept=Your settings, paired PCs, controller layouts, app lists, cover art and logs stay in %1, so installing Nightjar again picks them up.
en.RemoveSettings=&Remove them too (screenshots and statistics stay)
de.SettingsKept=Deine Einstellungen, gekoppelten PCs, Controller-Belegungen, App-Listen, Cover-Bilder und Protokolle bleiben in %1, damit eine neue Installation sie wieder verwendet.
de.RemoveSettings=&Diese auch entfernen (Screenshots und Statistiken bleiben)

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[InstallDelete]
; Only the license texts, which nothing needs to start. The libraries an upgrade no longer ships
; go after it succeeded (RemoveStaleDlls): this runs before [Files], and neither these deletions
; nor a file [Files] replaced is restored when the install aborts. Only over Nightjar's own previous
; install (the folder its uninstall entry names), never in a folder a first install was pointed at
; with /DIR.
Type: filesandordirs; Name: "{app}\licenses-*"; Check: IsUpgradeOfThisFolder

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Excludes: "\nightjar.exe"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "..\icons\nightjar.ico"; DestDir: "{app}"; Flags: ignoreversion
; Last: an install that aborts leaves the old program with every library it had, or a complete new one.
Source: "{#SourceDir}\nightjar.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Nightjar"; Filename: "{app}\nightjar.exe"; IconFilename: "{app}\nightjar.ico"; Comment: "Stream games from your PC"
Name: "{autodesktop}\Nightjar"; Filename: "{app}\nightjar.exe"; IconFilename: "{app}\nightjar.ico"; Comment: "Stream games from your PC"; Tasks: desktopicon

[Run]
Filename: "{app}\nightjar.exe"; Description: "{cm:LaunchProgram,Nightjar}"; Flags: nowait postinstall skipifsilent

[Code]
const
  // The entry Setup writes for the AppId above (per user, so under HKCU).
  UninstallKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{6A3C8F12-5E7B-4D2A-9C41-7B0E8D3F2A65}_is1';

var
  WasUpgrade: Boolean;

// True when {app} is the folder of the Nightjar this Setup upgrades.
function IsUpgradeOfThisFolder(): Boolean;
var
  Previous: String;
begin
  Result := RegQueryStringValue(HKCU, UninstallKey, 'InstallLocation', Previous) and
    (CompareText(RemoveBackslashUnlessRoot(Previous), RemoveBackslashUnlessRoot(ExpandConstant('{app}'))) = 0);
end;

// The folder above the client's data (directories' ProjectDirs for "Nightjar":
// %APPDATA%\Nightjar\config).
function DataDir(): String;
begin
#ifdef TestDataDir
  Result := '{#TestDataDir}';
#else
  Result := ExpandConstant('{userappdata}\Nightjar');
#endif
end;

function ConfigDir(): String;
begin
  Result := AddBackslash(DataDir()) + 'config';
end;

// The user's own files in the data folder: kept even when the settings go.
function IsUserFile(Name: String): Boolean;
begin
  Result := (CompareText(Name, 'screenshots') = 0) or (CompareText(Name, 'statistics') = 0);
end;

// Removes everything in the data folder but the user's own files, then the folders that leaves
// empty. False when something could not be removed.
function RemoveSettings(): Boolean;
var
  Found: TFindRec;
  Path: String;
begin
  Result := True;
  if FindFirst(ConfigDir() + '\*', Found) then begin
    try
      repeat
        Path := ConfigDir() + '\' + Found.Name;
        if (Found.Name <> '.') and (Found.Name <> '..') and not IsUserFile(Found.Name) then begin
          if (Found.Attributes and FILE_ATTRIBUTE_DIRECTORY) <> 0 then begin
            if not DelTree(Path, True, True, True) then begin
              Log('Could not remove ' + Path);
              Result := False;
            end;
          end else if not DeleteFile(Path) then begin
            Log('Could not remove ' + Path);
            Result := False;
          end;
        end;
      until not FindNext(Found);
    finally
      FindClose(Found);
    end;
  end;
  // RemoveDir only removes an empty folder: one with screenshots in it stays.
  RemoveDir(ConfigDir());
  RemoveDir(DataDir());
end;

// After the files are gone: say where the settings stay, and remove them only when asked.
function AskRemoveSettings(): Boolean;
var
  Form: TSetupForm;
  Text: TNewStaticText;
  Box: TNewCheckBox;
  Button: TNewButton;
begin
  Form := CreateCustomForm(ScaleX(500), ScaleY(176), False, True);
  try
    Form.Caption := 'Nightjar';
    Text := TNewStaticText.Create(Form);
    Text.Parent := Form;
    Text.Left := ScaleX(16);
    Text.Top := ScaleY(16);
    Text.Width := Form.ClientWidth - ScaleX(32);
    Text.AutoSize := False;
    Text.WordWrap := True;
    Text.Height := ScaleY(72);
    Text.Caption := FmtMessage(CustomMessage('SettingsKept'), [ConfigDir()]);
    Box := TNewCheckBox.Create(Form);
    Box.Parent := Form;
    Box.Left := ScaleX(16);
    Box.Top := ScaleY(96);
    Box.Width := Form.ClientWidth - ScaleX(32);
    Box.Height := ScaleY(20);
    Box.Caption := CustomMessage('RemoveSettings');
    Box.Checked := False;
    Button := TNewButton.Create(Form);
    Button.Parent := Form;
    Button.Width := ScaleX(88);
    Button.Height := ScaleY(26);
    Button.Left := Form.ClientWidth - Button.Width - ScaleX(16);
    Button.Top := Form.ClientHeight - Button.Height - ScaleY(14);
    Button.Caption := SetupMessage(msgButtonOK);
    Button.ModalResult := mrOk;
    Button.Default := True;
    Form.ActiveControl := Button;
    // Closing the box with X keeps everything, ticked or not.
    Result := (Form.ShowModal() = mrOk) and Box.Checked;
  finally
    Form.Free();
  end;
end;

// A silent uninstall keeps the settings unless its command line has /REMOVESETTINGS.
function RemoveSettingsParam(): Boolean;
var
  I: Integer;
begin
  Result := False;
  for I := 1 to ParamCount() do
    if CompareText(ParamStr(I), '/REMOVESETTINGS') = 0 then
      Result := True;
end;

// After a successful upgrade only: a DLL the new version no longer ships (another FFmpeg version)
// must not stay behind.
procedure RemoveStaleDlls();
var
  Found: TFindRec;
  Folder: String;
begin
  Folder := ExpandConstant('{app}\');
  if FindFirst(Folder + '*.dll', Found) then begin
    try
      repeat
        if (Pos('|' + Lowercase(Found.Name) + '|', '{#ShippedDlls}') = 0) and not DeleteFile(Folder + Found.Name) then
          Log('Could not remove the old ' + Found.Name);
      until not FindNext(Found);
    finally
      FindClose(Found);
    end;
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  // Asked before the install: it rewrites the entry IsUpgradeOfThisFolder reads.
  if CurStep = ssInstall then
    WasUpgrade := IsUpgradeOfThisFolder()
  else if (CurStep = ssPostInstall) and WasUpgrade then
    RemoveStaleDlls();
end;

// Nightjar's Update now (self_update.rs) exits and runs Setup silently with /RELAUNCH, so it needs
// Setup to bring it back: here, not in [Run], so that a Setup that failed brings it back too, on
// the old version, and Nightjar says so.
function RelaunchParam(): Boolean;
var
  I: Integer;
begin
  Result := False;
  for I := 1 to ParamCount() do
    if CompareText(ParamStr(I), '/RELAUNCH') = 0 then
      Result := True;
end;

procedure DeinitializeSetup();
var
  Folder: String;
  ResultCode: Integer;
begin
  if RelaunchParam() and RegQueryStringValue(HKCU, UninstallKey, 'InstallLocation', Folder) then
    if not Exec(AddBackslash(Folder) + 'nightjar.exe', '', Folder, SW_SHOWNORMAL, ewNoWait, ResultCode) then
      Log('Could not start Nightjar again: ' + SysErrorMessage(ResultCode));
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Remove: Boolean;
begin
  if (CurUninstallStep <> usPostUninstall) or not DirExists(ConfigDir()) then
    Exit;
  if UninstallSilent() then
    Remove := RemoveSettingsParam()
  else
    Remove := AskRemoveSettings();
  if Remove and not RemoveSettings() then begin
    Log(FmtMessage('Could not remove everything in %1.', [ConfigDir()]));
    MsgBox(FmtMessage('Could not remove everything in %1.', [ConfigDir()]), mbError, MB_OK);
  end;
end;
