; The Nightjar for Windows installer (Inno Setup 6). build-installer.ps1 compiles it from the
; unpacked release zip, so the installer carries exactly the zip's files:
;
;   ISCC.exe /DAppVersion=4.0.0 /DNumericVersion=4.0.0.0 /DSourceDir=<unpacked zip>
;            /DOutputDir=<folder> /DOutputBaseName=<name> nightjar.iss
;
; Per user, no administrator prompt: %LOCALAPPDATA%\Programs\Nightjar, a Start menu shortcut, an
; optional desktop shortcut and an Apps & features entry. The settings, the paired PCs and the
; logs live in %APPDATA%\Nightjar (config.rs, config_dir), outside the install folder: an upgrade
; never touches them, and an uninstall removes them only when the user ticks the box for it (or
; passes /REMOVESETTINGS to a silent uninstall).

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
en.SettingsKept=Your settings and paired PCs stay in %1, so installing Nightjar again picks them up.
en.RemoveSettings=&Remove my settings and paired PCs too
de.SettingsKept=Deine Einstellungen und gekoppelten PCs bleiben in %1, damit eine neue Installation sie wieder verwendet.
de.RemoveSettings=&Meine Einstellungen und gekoppelten PCs auch entfernen

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[InstallDelete]
; An upgrade replaces the libraries: a DLL the new version no longer ships (another FFmpeg
; version) must not stay behind. Only files the package itself brings are matched.
Type: files; Name: "{app}\*.dll"
Type: filesandordirs; Name: "{app}\licenses-*"

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "..\icons\nightjar.ico"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Nightjar"; Filename: "{app}\nightjar.exe"; IconFilename: "{app}\nightjar.ico"; Comment: "Stream games from your PC"
Name: "{autodesktop}\Nightjar"; Filename: "{app}\nightjar.exe"; IconFilename: "{app}\nightjar.ico"; Comment: "Stream games from your PC"; Tasks: desktopicon

[Run]
Filename: "{app}\nightjar.exe"; Description: "{cm:LaunchProgram,Nightjar}"; Flags: nowait postinstall skipifsilent

[Code]
// The client's data folder (directories' ProjectDirs for "Nightjar": %APPDATA%\Nightjar\config).
function DataDir(): String;
begin
  Result := ExpandConstant('{userappdata}\Nightjar');
end;

// After the files are gone: say where the settings stay, and remove them only when asked.
function AskRemoveSettings(): Boolean;
var
  Form: TSetupForm;
  Text: TNewStaticText;
  Box: TNewCheckBox;
  Button: TNewButton;
begin
  Form := CreateCustomForm(ScaleX(420), ScaleY(150), False, True);
  try
    Form.Caption := 'Nightjar';
    Text := TNewStaticText.Create(Form);
    Text.Parent := Form;
    Text.Left := ScaleX(16);
    Text.Top := ScaleY(16);
    Text.Width := Form.ClientWidth - ScaleX(32);
    Text.AutoSize := False;
    Text.WordWrap := True;
    Text.Height := ScaleY(48);
    Text.Caption := FmtMessage(CustomMessage('SettingsKept'), [DataDir()]);
    Box := TNewCheckBox.Create(Form);
    Box.Parent := Form;
    Box.Left := ScaleX(16);
    Box.Top := ScaleY(72);
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
    Button.Cancel := True;
    Form.ActiveControl := Button;
    Form.ShowModal();
    Result := Box.Checked;
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

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Remove: Boolean;
begin
  if (CurUninstallStep <> usPostUninstall) or not DirExists(DataDir()) then
    Exit;
  if UninstallSilent() then
    Remove := RemoveSettingsParam()
  else
    Remove := AskRemoveSettings();
  if Remove and not DelTree(DataDir(), True, True, True) then begin
    Log(FmtMessage('Could not remove %1 completely.', [DataDir()]));
    MsgBox(FmtMessage('Could not remove %1 completely.', [DataDir()]), mbError, MB_OK);
  end;
end;
