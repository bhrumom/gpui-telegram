; Inno Setup installer script for Fabushi Desktop (Windows x64)
; Invoked by scripts/package_windows.py with /DAppVersion, /DStageDir, /DOutDir, /DOutBaseFilename, /DIconPath

#ifndef AppVersion
  #define AppVersion "1.7.1"
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
ChangesEnvironment=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"
Name: "addtopath"; Description: "Add Fabushi CLI (fabushi) to PATH"; GroupDescription: "CLI & Background Services:"

[Files]
Source: "{#StageDir}\Fabushi.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StageDir}\bin\fabushi.exe"; DestDir: "{app}\bin"; Flags: ignoreversion skipifsourcedoesntexist
Source: "{#StageDir}\bin\fabushi-remote-daemon.exe"; DestDir: "{app}\bin"; Flags: ignoreversion skipifsourcedoesntexist
Source: "{#StageDir}\README.txt"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist

[Icons]
Name: "{group}\Fabushi"; Filename: "{app}\Fabushi.exe"
Name: "{group}\{cm:UninstallProgram,Fabushi}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\Fabushi"; Filename: "{app}\Fabushi.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\Fabushi.exe"; Description: "{cm:LaunchProgram,Fabushi}"; Flags: nowait postinstall skipifsilent

[Code]
procedure GetEnvRegLocation(var RootKey: Integer; var SubKey: String);
begin
  if IsAdminInstallMode then
  begin
    RootKey := HKEY_LOCAL_MACHINE;
    SubKey := 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment';
  end
  else
  begin
    RootKey := HKEY_CURRENT_USER;
    SubKey := 'Environment';
  end;
end;

procedure EnvAddPath(PathToAdd: String);
var
  RootKey: Integer;
  SubKey: String;
  Paths: String;
  P: Integer;
  Entry: String;
  Remaining: String;
begin
  GetEnvRegLocation(RootKey, SubKey);
  if not RegQueryStringValue(RootKey, SubKey, 'Path', Paths) then
    Paths := '';

  Remaining := Paths;
  while Remaining <> '' do
  begin
    P := Pos(';', Remaining);
    if P > 0 then
    begin
      Entry := Trim(Copy(Remaining, 1, P - 1));
      Remaining := Copy(Remaining, P + 1, Length(Remaining) - P);
    end
    else
    begin
      Entry := Trim(Remaining);
      Remaining := '';
    end;
    if (Entry <> '') and (CompareText(Entry, PathToAdd) = 0) then
      exit;
  end;

  if (Paths <> '') and (Paths[Length(Paths)] <> ';') then
    Paths := Paths + ';';
  Paths := Paths + PathToAdd;
  RegWriteExpandStringValue(RootKey, SubKey, 'Path', Paths);
end;

procedure EnvRemovePath(PathToRemove: String);
var
  RootKey: Integer;
  SubKey: String;
  Paths: String;
  NewPaths: String;
  P: Integer;
  Entry: String;
  Remaining: String;
  Changed: Boolean;
begin
  GetEnvRegLocation(RootKey, SubKey);
  if not RegQueryStringValue(RootKey, SubKey, 'Path', Paths) then
    exit;

  NewPaths := '';
  Remaining := Paths;
  Changed := False;
  while Remaining <> '' do
  begin
    P := Pos(';', Remaining);
    if P > 0 then
    begin
      Entry := Trim(Copy(Remaining, 1, P - 1));
      Remaining := Copy(Remaining, P + 1, Length(Remaining) - P);
    end
    else
    begin
      Entry := Trim(Remaining);
      Remaining := '';
    end;
    if Entry = '' then
      continue;
    if CompareText(Entry, PathToRemove) = 0 then
    begin
      Changed := True;
      continue;
    end;
    if NewPaths <> '' then
      NewPaths := NewPaths + ';';
    NewPaths := NewPaths + Entry;
  end;

  if Changed then
  begin
    RegWriteExpandStringValue(RootKey, SubKey, 'Path', NewPaths);
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
  begin
    if WizardIsTaskSelected('addtopath') then
      EnvAddPath(ExpandConstant('{app}\bin'));
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
  begin
    EnvRemovePath(ExpandConstant('{app}\bin'));
  end;
end;
