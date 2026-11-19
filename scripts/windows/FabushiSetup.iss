; Inno Setup installer script for Fabushi Desktop (Windows x64)
; Invoked by scripts/package_windows.py with /DAppVersion, /DStageDir, /DOutDir, /DOutBaseFilename, /DIconPath

#ifndef AppVersion
  #define AppVersion "1.3.2"
#endif
#ifndef StageDir
  #define StageDir "..\..\dist\windows-stage"
#endif
#ifndef OutDir
  #define OutDir "..\..\dist"
#endif
#ifndef OutBaseFilename
  #define OutBaseFilename "Fabushi-" + AppVersion + "-windows-x86_64-setup"
#endif
#ifndef IconPath
  #define IconPath "..\..\crates\gpui_telegram_desktop\assets\AppIcon.ico"
#endif

[Setup]
AppId={{8E92D6F1-7C4B-4F3A-9C35-2F7A61E9B4C2}
AppName=Fabushi
AppVersion={#AppVersion}
AppVerName=Fabushi {#AppVersion}
AppPublisher=Fabushi Contributors
AppPublisherURL=https://github.com/bhrumom/gpui-telegram
AppSupportURL=https://github.com/bhrumom/gpui-telegram/releases
AppUpdatesURL=https://github.com/bhrumom/gpui-telegram/releases
DefaultDirName={autopf}\Fabushi
DefaultGroupName=Fabushi
AllowNoIcons=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#OutDir}
OutputBaseFilename={#OutBaseFilename}
SetupIconFile={#IconPath}
UninstallDisplayIcon={app}\Fabushi.exe
UninstallDisplayName=Fabushi
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no
ChangesEnvironment=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "{#StageDir}\Fabushi.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\gpui-tg-cli.exe"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist
Source: "{#StageDir}\README.txt"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist

[Icons]
Name: "{group}\Fabushi"; Filename: "{app}\Fabushi.exe"
Name: "{group}\{cm:UninstallProgram,Fabushi}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\Fabushi"; Filename: "{app}\Fabushi.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\Fabushi.exe"; Description: "{cm:LaunchProgram,Fabushi}"; Flags: nowait postinstall skipifsilent
