#define MyAppName "VideoLicker"
#define MyAppVersion "0.1.0"
#define MyAppPublisher "Thomas"
#define MyAppExeName "tubegrab.exe"

[Setup]
AppId={{B62CF013-1E9E-47A3-BA40-4C2E0F97C821}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
DefaultDirName={autopf}\TubeGrab
DefaultGroupName=VideoLicker
OutputDir=dist
OutputBaseFilename=VideoLicker-Setup-{#MyAppVersion}
ArchitecturesInstallIn64BitMode=x64
PrivilegesRequired=admin
UninstallDisplayIcon={app}\{#MyAppExeName}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern

[Files]
Source: "target\release\VideoLicker.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\VideoLicker"; Filename: "{app}\{#MyAppExeName}"
Name: "{autodesktop}\VideoLicker"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Additional shortcuts:"

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "Launch VideoLicker"; Flags: postinstall nowait skipifsilent