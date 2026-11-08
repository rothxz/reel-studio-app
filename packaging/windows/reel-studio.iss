; The Windows installer (Inno Setup 6), built by packaging/windows/bundle.ps1:
;   iscc /DVersion=<v> /DArch=<x64> /DSource=<dir with reel-studio.exe>
;        /DIcon=<app .ico> /DOutDir=<out> reel-studio.iss
;
; A per-user install (no administrator rights) in
; %LOCALAPPDATA%\Programs\Reel Studio: a folder the user can write to, so
; the in-app updater can replace reel-studio.exe there, like the zipped
; copy. Start menu entry, optional desktop shortcut, an uninstaller.

#define AppName "Reel Studio"
#define AppExe "reel-studio.exe"

[Setup]
; Keep this id: it is how an upgrade finds the installed copy.
AppId={{65239B2C-EEB1-4AF9-B75F-EF5AD995DB3D}
AppName={#AppName}
AppVersion={#Version}
AppVerName={#AppName} {#Version}
AppPublisher={#AppName}
DefaultDirName={localappdata}\Programs\{#AppName}
DisableDirPage=auto
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir={#OutDir}
OutputBaseFilename=Reel-Studio-{#Version}-windows-{#Arch}-setup
SetupIconFile={#Icon}
UninstallDisplayIcon={app}\{#AppExe}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; Ask a running copy to close first (it saves its project on close).
CloseApplications=yes

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; Flags: unchecked

[Files]
Source: "{#Source}\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExe}"; Description: "Open {#AppName}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; What an update leaves beside the program until the next launch.
Type: files; Name: "{app}\.{#AppExe}.old"
Type: files; Name: "{app}\.{#AppExe}.incoming"
