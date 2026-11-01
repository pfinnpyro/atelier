; Inno Setup script for Atelier. The release workflow builds it with
;   ISCC.exe /DAppVersion=1.0.0 installer\atelier.iss
; Installs for the current user by default (no admin prompt), so the
; in-app updater can replace the program without elevation. Offers a
; per-machine install too.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

#ifndef BinaryDir
  #define BinaryDir "..\target\x86_64-pc-windows-msvc\release"
#endif

[Setup]
AppId={{919EDE17-034D-46D1-969D-793A3520E45C}
AppName=Atelier
AppVersion={#AppVersion}
AppVerName=Atelier {#AppVersion}
AppPublisher=Patrick Finnegan
AppPublisherURL=https://github.com/pfinnpyro/atelier
AppUpdatesURL=https://github.com/pfinnpyro/atelier/releases
DefaultDirName={autopf}\Atelier
DefaultGroupName=Atelier
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=Output
OutputBaseFilename=atelier-setup
SetupIconFile=..\crates\atelier-app\assets\atelier.ico
UninstallDisplayIcon={app}\atelier-viewer.exe
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; Lets the updater run it silently while Atelier is open.
CloseApplications=yes
RestartApplications=no
ChangesAssociations=yes

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Shortcuts:"; Flags: unchecked
Name: "pdfassoc"; Description: "Offer Atelier in &Open with for PDF files"; GroupDescription: "PDF files:"

[Files]
Source: "{#BinaryDir}\atelier-viewer.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#BinaryDir}\atelier.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Atelier"; Filename: "{app}\atelier-viewer.exe"
Name: "{autodesktop}\Atelier"; Filename: "{app}\atelier-viewer.exe"; Tasks: desktopicon

[Registry]
; The same keys the app's "Make Atelier the default PDF app" writes, so
; Windows lists Atelier under Default apps. HKA is HKCU for a per-user
; install and HKLM for a per-machine one.
Root: HKA; Subkey: "Software\Classes\Atelier.PDF"; ValueType: string; ValueName: ""; ValueData: "PDF Document"; Flags: uninsdeletekey; Tasks: pdfassoc
Root: HKA; Subkey: "Software\Classes\Atelier.PDF\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\atelier-viewer.exe,0"; Tasks: pdfassoc
Root: HKA; Subkey: "Software\Classes\Atelier.PDF\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\atelier-viewer.exe"" ""%1"""; Tasks: pdfassoc
Root: HKA; Subkey: "Software\Classes\.pdf\OpenWithProgids"; ValueType: none; ValueName: "Atelier.PDF"; Flags: uninsdeletevalue; Tasks: pdfassoc
Root: HKA; Subkey: "Software\Atelier\Capabilities"; ValueType: string; ValueName: "ApplicationName"; ValueData: "Atelier"; Flags: uninsdeletekey; Tasks: pdfassoc
Root: HKA; Subkey: "Software\Atelier\Capabilities"; ValueType: string; ValueName: "ApplicationDescription"; ValueData: "View, fill in and inspect PDF files."; Tasks: pdfassoc
Root: HKA; Subkey: "Software\Atelier\Capabilities\FileAssociations"; ValueType: string; ValueName: ".pdf"; ValueData: "Atelier.PDF"; Tasks: pdfassoc
Root: HKA; Subkey: "Software\RegisteredApplications"; ValueType: string; ValueName: "Atelier"; ValueData: "Software\Atelier\Capabilities"; Flags: uninsdeletevalue; Tasks: pdfassoc

[Run]
Filename: "{app}\atelier-viewer.exe"; Description: "Open Atelier"; Flags: nowait postinstall skipifsilent
; After a silent update started from inside Atelier, open the new version.
Filename: "{app}\atelier-viewer.exe"; Flags: nowait; Check: IsUpdate

[Code]
function IsUpdate: Boolean;
begin
  Result := ExpandConstant('{param:update|0}') = '1';
end;
