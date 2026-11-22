; Inno Setup script for Vireo. Built by installer/build.ps1 from a staged release folder.
; Written for Inno Setup 6.3 or later. CI installs 6.7.1 from Chocolatey; keep that pin
; (in .github/workflows) in step with this script.

#define MyAppName "Vireo"
#ifndef MyAppVersion
  #define MyAppVersion "0.0.0"
#endif
#define MyAppPublisher "Cosmos-0118"
#define MyAppExeName "vireo.exe"
#ifndef BuildOutput
  #define BuildOutput "..\artifacts\publish"
#endif

[Setup]
AppId={{A4E9C2F1-7B38-4D15-9C62-8E1B4F3A7D20}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL=https://github.com/Cosmos-0118/vireo
AppSupportURL=https://github.com/Cosmos-0118/vireo/issues
AppUpdatesURL=https://github.com/Cosmos-0118/vireo/releases
; Version resource of Setup itself so Properties > Details shows the release version.
; installer/build.ps1 enforces a three-part version; the fourth part is always 0.
VersionInfoVersion={#MyAppVersion}.0
VersionInfoProductVersion={#MyAppVersion}.0
VersionInfoTextVersion={#MyAppVersion}
VersionInfoProductTextVersion={#MyAppVersion}
VersionInfoCompany={#MyAppPublisher}
VersionInfoProductName={#MyAppName}
VersionInfoDescription={#MyAppName} Setup
VersionInfoCopyright=MIT License
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
OutputBaseFilename=Vireo-Setup-{#MyAppVersion}
SetupIconFile=..\backend\icons\icon.ico
UninstallDisplayIcon={app}\{#MyAppExeName}
LicenseFile=..\LICENSE
; One solid LZMA2 stream with a 64 MB dictionary: the bundled MinGit is hundreds of small,
; similar files, so this is markedly smaller than per-file compression.
Compression=lzma2/ultra64
SolidCompression=yes
LZMAUseSeparateProcess=yes
LZMANumBlockThreads=4
WizardStyle=modern
; Per-user install by default (no UAC needed, like VS Code User Setup).
; Users or admins can still choose an all-users install from the dialog or with /ALLUSERS.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog commandline
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.19041
ChangesEnvironment=yes
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "addtopath"; Description: "Add {#MyAppName} (vireo and vx) to &PATH"; GroupDescription: "Command line:"
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Additional shortcuts:"; Flags: unchecked

[Files]
Source: "{#BuildOutput}\*"; DestDir: "{app}"; Flags: recursesubdirs ignoreversion

[Icons]
Name: "{autoprograms}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; WorkingDir: "{app}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon; WorkingDir: "{app}"

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "Launch {#MyAppName}"; Flags: postinstall skipifsilent nowait

[InstallDelete]
; Remove files from an earlier build only when the chosen folder was registered by a
; previous Vireo Setup, so an unrelated folder picked by mistake is never emptied.
Type: filesandordirs; Name: "{app}\*"; Check: AppFolderIsOurs

[Code]
function RegisteredInstallFolder(const Root: Integer): string;
var
  Value: string;
begin
  Result := '';
  if RegQueryStringValue(Root, ExpandConstant('Software\Microsoft\Windows\CurrentVersion\Uninstall\{#emit SetupSetting("AppId")}_is1'), 'InstallLocation', Value) then
    Result := RemoveBackslashUnlessRoot(Value);
end;

function AppFolderIsOurs: Boolean;
var
  Folder: string;
begin
  Folder := RemoveBackslashUnlessRoot(ExpandConstant('{app}'));
  Result := FileExists(AddBackslash(Folder) + '{#MyAppExeName}') and
    ((CompareText(RegisteredInstallFolder(HKLM), Folder) = 0) or (CompareText(RegisteredInstallFolder(HKCU), Folder) = 0));
end;

procedure GetEnvPathKey(out RootKey: Integer; out SubKey: string);
begin
  if IsAdminInstallMode then
  begin
    RootKey := HKLM;
    SubKey := 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment';
  end
  else
  begin
    RootKey := HKCU;
    SubKey := 'Environment';
  end;
end;

function PathContainsDir(const PathValue, Dir: string): Boolean;
var
  NormalizedDir, Remaining, Entry: string;
  SepPos: Integer;
begin
  NormalizedDir := RemoveBackslashUnlessRoot(Dir);
  Remaining := PathValue;
  Result := False;
  while Remaining <> '' do
  begin
    SepPos := Pos(';', Remaining);
    if SepPos > 0 then
    begin
      Entry := Copy(Remaining, 1, SepPos - 1);
      Delete(Remaining, 1, SepPos);
    end
    else
    begin
      Entry := Remaining;
      Remaining := '';
    end;
    if CompareText(RemoveBackslashUnlessRoot(Trim(Entry)), NormalizedDir) = 0 then
    begin
      Result := True;
      Exit;
    end;
  end;
end;

procedure AddDirToPath(const Dir: string);
var
  RootKey: Integer;
  SubKey, CurrentPath, CleanDir, NewPath: string;
begin
  GetEnvPathKey(RootKey, SubKey);
  CleanDir := RemoveBackslashUnlessRoot(Dir);
  if not RegQueryStringValue(RootKey, SubKey, 'Path', CurrentPath) then
    CurrentPath := '';
  if PathContainsDir(CurrentPath, CleanDir) then
    Exit;
  if CurrentPath = '' then
    NewPath := CleanDir
  else if CurrentPath[Length(CurrentPath)] = ';' then
    NewPath := CurrentPath + CleanDir
  else
    NewPath := CurrentPath + ';' + CleanDir;
  RegWriteExpandStringValue(RootKey, SubKey, 'Path', NewPath);
end;

procedure RemoveDirFromPath(const Dir: string);
var
  RootKey: Integer;
  SubKey, CurrentPath, CleanDir, Remaining, Entry, Updated: string;
  SepPos: Integer;
begin
  GetEnvPathKey(RootKey, SubKey);
  if not RegQueryStringValue(RootKey, SubKey, 'Path', CurrentPath) then
    Exit;
  CleanDir := RemoveBackslashUnlessRoot(Dir);
  if not PathContainsDir(CurrentPath, CleanDir) then
    Exit;
  Remaining := CurrentPath;
  Updated := '';
  while Remaining <> '' do
  begin
    SepPos := Pos(';', Remaining);
    if SepPos > 0 then
    begin
      Entry := Copy(Remaining, 1, SepPos - 1);
      Delete(Remaining, 1, SepPos);
    end
    else
    begin
      Entry := Remaining;
      Remaining := '';
    end;
    if (Trim(Entry) <> '') and (CompareText(RemoveBackslashUnlessRoot(Trim(Entry)), CleanDir) <> 0) then
    begin
      if Updated <> '' then
        Updated := Updated + ';';
      Updated := Updated + Entry;
    end;
  end;
  RegWriteExpandStringValue(RootKey, SubKey, 'Path', Updated);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssPostInstall) and WizardIsTaskSelected('addtopath') then
    AddDirToPath(ExpandConstant('{app}'));
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    RemoveDirFromPath(ExpandConstant('{app}'));
end;
