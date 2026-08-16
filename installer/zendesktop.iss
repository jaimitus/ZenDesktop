; ZenDesktop Installer (Inno Setup)
; Build: ISCC.exe /DMyAppVersion=1.0.27 zendesktop.iss
; Produces: ..\release\ZenDesktop-1.0.27-setup.exe

#ifndef MyAppVersion
  #define MyAppVersion "1.0.27"
#endif

[Setup]
AppId={{8F7C2E4D-9A1B-4C3D-8E5F-6A7B8C9D0E1F}
AppName=ZenDesktop
AppVersion={#MyAppVersion}
AppVerName=ZenDesktop {#MyAppVersion}
AppPublisher=ZenDesktop
AppPublisherURL=https://github.com/jaimitus/ZenDesktop
AppSupportURL=https://github.com/jaimitus/ZenDesktop
AppUpdatesURL=https://github.com/jaimitus/ZenDesktop
DefaultDirName={autopf}\ZenDesktop
DefaultGroupName=ZenDesktop
DisableProgramGroupPage=yes
OutputDir=..\release
OutputBaseFilename=ZenDesktop-{#MyAppVersion}-setup
SetupIconFile=staging\zendesktop.ico
UninstallDisplayIcon={app}\ZenDesktop.exe
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=admin
LicenseFile=staging\LICENSE

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "staging\ZenDesktop.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "staging\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "staging\README.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\ZenDesktop"; Filename: "{app}\ZenDesktop.exe"
Name: "{autodesktop}\ZenDesktop"; Filename: "{app}\ZenDesktop.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\ZenDesktop.exe"; Description: "{cm:LaunchProgram,ZenDesktop}"; Flags: nowait postinstall skipifsilent
