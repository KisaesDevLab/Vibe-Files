; Vibe Files: Windows installer (Inno Setup 6). PHASES PA-102, owner 2026-11-23.
; Modelled on ClientLedger Trial Balance's installer (same choice of who it is for).
;
; Built by tools/release.ps1 from the folder it staged for the portable zip:
;   ISCC.exe /DStageDir=<stage> /DOutputDir=<dist\v<ver>> /DAppVersion=<ver> installer\VibeFiles.iss
;
; Setup asks "Install for me only" (the default, no administrator rights):
;   %LOCALAPPDATA%\Programs\Vibe Files
; or "Install for all users" (asks for administrator): %ProgramFiles%\Vibe Files.
; Silent installs choose with /CURRENTUSER or /ALLUSERS. An upgrade keeps the earlier choice.
;
; Either way each Windows login's data (catalog, settings, caches, logs) is in
; %LOCALAPPDATA%\Vibe Files\data: installed.txt beside the exe tells the program it is installed
; (vf-app datalock::INSTALLED_MARKER). Install, upgrade and uninstall never touch the data or the
; cabinet. A firm.toml an administrator puts beside the exe is not Setup's, so upgrade and
; uninstall leave it.

#ifndef StageDir
  #error Pass /DStageDir=<the staged portable folder> (tools/release.ps1 does)
#endif
#ifndef OutputDir
  #define OutputDir AddBackslash(SourcePath) + "..\target\dist"
#endif
#ifndef AppVersion
  #define CargoToml AddBackslash(SourcePath) + "..\Cargo.toml"
  #define AppVersion StringChange(ReadIni(CargoToml, "workspace.package", "version", ""), '"', "")
  #if AppVersion == ""
    #error No [workspace.package] version found in Cargo.toml
  #endif
#endif
#if Pos("-", AppVersion) > 0
  #define AppVersionNumeric Copy(AppVersion, 1, Pos("-", AppVersion) - 1)
#else
  #define AppVersionNumeric AppVersion
#endif

#define AppName "Vibe Files"
#define AppExe "vibe-files.exe"
#define PubRepo "https://github.com/KisaesDevLab/vibe-files-releases"

[Setup]
; Never change AppId: upgrades find the earlier install by it.
AppId={{BB477148-B4C8-4E96-BF83-20AAAA31668B}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=Kisaes LLC
AppPublisherURL={#PubRepo}
AppSupportURL={#PubRepo}/issues
AppUpdatesURL={#PubRepo}/releases
AppContact=support@kisaes.com
AppCopyright=Copyright 2026 Kisaes LLC
VersionInfoVersion={#AppVersionNumeric}.0
VersionInfoCompany=Kisaes LLC
VersionInfoProductName={#AppName}
VersionInfoProductTextVersion={#AppVersion}
VersionInfoDescription={#AppName} Setup
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog commandline
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.19045
DefaultDirName={autopf}\{#AppName}
UsePreviousAppDir=yes
DisableProgramGroupPage=yes
LicenseFile={#StageDir}\LICENSE.txt
SetupIconFile={#SourcePath}..\crates\vf-app\assets\vibe-files.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
OutputDir={#OutputDir}
OutputBaseFilename=vibe-files-setup-x64
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[InstallDelete]
; An upgrade replaces the bundled Tesseract whole (no DLL of an older one is left behind).
Type: filesandordirs; Name: "{app}\tesseract"
Type: filesandordirs; Name: "{app}\pdfium-licenses"

[Files]
Source: "{#StageDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "installed.txt"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"; WorkingDir: "{app}"; Comment: "Client documents over your firm's folder tree"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; WorkingDir: "{app}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent

; No [UninstallDelete]: the uninstaller removes only the files it installed. The data folder and
; the cabinet are never named here.

[Code]
// Only this install's copies count (its folder), not a portable copy run from elsewhere.
function AppIsRunning(Dir: String): Boolean;
var
  Locator, Service, Found, Proc, Exe: Variant;
  I: Integer;
  Path, Prefix: String;
begin
  Result := False;
  Prefix := Lowercase(AddBackslash(Dir));
  try
    Locator := CreateOleObject('WbemScripting.SWbemLocator');
    Service := Locator.ConnectServer('.', 'root\CIMV2');
    Found := Service.ExecQuery('SELECT ExecutablePath FROM Win32_Process WHERE Name = ''vibe-files.exe'' OR Name = ''vibe-move.exe'' OR Name = ''vibe-scan32.exe'' OR Name = ''tesseract.exe''');
    for I := 0 to Found.Count - 1 do
    begin
      Proc := Found.ItemIndex(I);
      Exe := Proc.ExecutablePath;
      if not VarIsNull(Exe) then
      begin
        Path := Lowercase(String(Exe));
        if Pos(Prefix, Path) = 1 then
        begin
          Result := True;
          Exit;
        end;
      end;
    end;
  except
    Result := False;
  end;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := '';
  if AppIsRunning(ExpandConstant('{app}')) then
    Result := '{#AppName} is open. Close it, then run Setup again.' + #13#10 +
              'Your cabinet and settings are not changed by this installer.';
end;

function InitializeUninstall(): Boolean;
begin
  Result := True;
  while AppIsRunning(ExpandConstant('{app}')) do
  begin
    if UninstallSilent() then
    begin
      Log('{#AppName} is running; stopping.');
      Result := False;
      Exit;
    end;
    if MsgBox('{#AppName} is open. Close it, then click OK.' + #13#10 + #13#10 +
              'Uninstalling does not remove your cabinet, documents or settings.',
              mbInformation, MB_OKCANCEL) = IDCANCEL then
    begin
      Result := False;
      Exit;
    end;
  end;
end;
