#ifndef AppVersion
  #error AppVersion is required
#endif
#ifndef BundleDir
  #error BundleDir is required
#endif
#ifndef OutputPath
  #error OutputPath is required
#endif
[Setup]
AppId={{56DAD926-7445-47AB-9FB7-9810D269BA7E}
AppName=外设盒
AppVersion={#AppVersion}
AppPublisher=外设盒 contributors
AppPublisherURL=https://github.com/chenran0408/peripheral-desk
AppUpdatesURL=https://github.com/chenran0408/peripheral-desk/releases
DefaultDirName={localappdata}\Programs\PeripheralDesk
DefaultGroupName=外设盒
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir={#OutputPath}
OutputBaseFilename=PeripheralDesk-{#AppVersion}-windows-x64-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\PeripheralDesk.exe
CloseApplications=yes
RestartApplications=no
LicenseFile={#BundleDir}\LICENSE
[Languages]
Name: "chinesesimp"; MessagesFile: "Languages\ChineseSimplified.isl"
[Tasks]
Name: desktopicon; Description: "创建桌面快捷方式"; Flags: unchecked
[Files]
Source: "{#BundleDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs
[Icons]
Name: "{autoprograms}\外设盒"; Filename: "{app}\PeripheralDesk.exe"; WorkingDir: "{app}"
Name: "{autodesktop}\外设盒"; Filename: "{app}\PeripheralDesk.exe"; WorkingDir: "{app}"; Tasks: desktopicon
[Run]
Filename: "{app}\PeripheralDesk.exe"; Description: "打开外设盒"; Flags: nowait; Check: ShouldLaunchApp
; No UninstallDelete entries: user-created data, downloaded plugins, and backups remain.

[Code]
var
  WasRunning: Boolean;
function InitializeSetup(): Boolean;
begin
  WasRunning := FindWindowByClassName('PeripheralDeskWindow') <> 0;
  Result := True;
end;
function ShouldLaunchApp(): Boolean;
begin
  Result := (not WizardSilent) or WasRunning or (ExpandConstant('{param:RESTARTAPP|0}') = '1');
end;

function InitializeUninstall(): Boolean;
begin
  Result := not CheckForMutexes('Local\PeripheralDesk');
  if not Result then
    SuppressibleMsgBox('请先从托盘退出外设盒，再运行卸载程序。', mbInformation, MB_OK, IDOK);
end;
