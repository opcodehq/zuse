; Per-user installation; removing the app never removes user profiles.
[Setup]
AppId={{9F205C0E-713D-4D26-8F91-835151E78F9A}
AppName=Zuse
AppVersion={#ReleaseVersion}
VersionInfoVersion={#NumericVersion}
AppPublisher=OpcodeHQ
AppPublisherURL=https://github.com/opcodehq/zuse
DefaultDirName={localappdata}\Programs\Zuse
DefaultGroupName=Zuse
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#OutputDir}
OutputBaseFilename=zuse-{#ReleaseVersion}-windows-x86_64-setup
SetupIconFile={#IconFile}
UninstallDisplayIcon={app}\zuse.exe
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\Zuse"; Filename: "{app}\zuse.exe"
Name: "{autodesktop}\Zuse"; Filename: "{app}\zuse.exe"; Tasks: desktopicon

[Tasks]
Name: desktopicon; Description: "Create a desktop shortcut"; Flags: unchecked

[Registry]
Root: HKCU; Subkey: "Software\Classes\zuse"; ValueType: string; ValueData: "URL:Zuse Protocol"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\zuse"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""
Root: HKCU; Subkey: "Software\Classes\zuse\shell\open\command"; ValueType: string; ValueData: """{app}\zuse.exe"" ""%1"""
