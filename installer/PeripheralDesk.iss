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
AppName=Peripheral Desk
AppVersion={#AppVersion}
AppPublisher=Peripheral Desk contributors
AppPublisherURL=https://github.com/chenran0408/peripheral-desk
AppUpdatesURL=https://github.com/chenran0408/peripheral-desk/releases
DefaultDirName={localappdata}\Programs\PeripheralDesk
DefaultGroupName=Peripheral Desk
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
AppMutex=Local\PeripheralDesk
CloseApplications=yes
RestartApplications=no
LicenseFile={#BundleDir}\LICENSE
[Tasks]
Name: desktopicon; Description: "Create a desktop shortcut"; Flags: unchecked
[Files]
Source: "{#BundleDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs
[Icons]
Name: "{autoprograms}\Peripheral Desk"; Filename: "{app}\PeripheralDesk.exe"; WorkingDir: "{app}"
Name: "{autodesktop}\Peripheral Desk"; Filename: "{app}\PeripheralDesk.exe"; WorkingDir: "{app}"; Tasks: desktopicon
[Run]
Filename: "{app}\PeripheralDesk.exe"; Description: "Launch Peripheral Desk"; Flags: nowait postinstall skipifsilent
; No UninstallDelete entries: user-created data, downloaded plugins, and backups remain.
