; Inno Setup script for the SoulCrafted Slicer Windows installer.
;
; The payload is a staging directory assembled by build.ps1 (both
; executables, the app-local VC runtime DLLs, and the bundled profiles
; laid out as the runtime expects). The script packs that directory and
; never references the build toolchain, so it is decoupled from the
; Visual Studio and Skia paths the build used.
;
; build.ps1 passes the version, the staging directory, and the output
; directory as preprocessor defines; the defaults below only exist so
; the script still compiles when opened directly in the Inno IDE.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef StageDir
  #define StageDir ".cache\stage"
#endif
#ifndef OutputDir
  #define OutputDir ".cache"
#endif

#define AppName "SoulCrafted Slicer"
#define GuiExe "sc-slicer-gui.exe"
#define CliExe "sc-slicer-cli.exe"

[Setup]
AppId=it.soulcrafted.Slicer
AppName={#AppName}
AppVersion={#AppVersion}
VersionInfoVersion={#AppVersion}
AppPublisher=SoulCrafted
AppPublisherURL=https://codeberg.org/SoulCrafted/Slicer
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
OutputDir={#OutputDir}
OutputBaseFilename=sc-slicer-setup-{#AppVersion}-x64
LicenseFile=..\..\LICENSE.md
SetupIconFile=..\..\apps\slicer-gui\assets\icons\it.soulcrafted.Slicer.ico
UninstallDisplayIcon={app}\{#GuiExe}
UninstallDisplayName={#AppName}
WizardStyle=modern
Compression=lzma2
SolidCompression=yes
; x64-only payload; install into the native 64-bit locations and refuse
; to run on anything that cannot execute x64 code.
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Let the user pick per-machine (elevated) or per-user (no elevation) at
; install time instead of forcing one scope.
PrivilegesRequired=admin
PrivilegesRequiredOverridesAllowed=dialog
; The script writes a per-user area (the HKCU PATH entry), which Inno warns
; about under an admin default. That entry is guarded to run only in
; per-user install mode (Check: not IsAdminInstallMode), so it never runs
; against an administrator's profile; silence the otherwise-correct warning.
UsedUserAreasWarning=no
; The PATH edits below change the environment; this makes Setup broadcast
; the change so new processes see it without a sign-out.
ChangesEnvironment=yes

; Wizard languages. Limited to the locales the application ships with
; enough translation coverage to appear in its own language selector and
; for which Inno Setup provides a wizard translation. Inno auto-selects
; the entry matching the user's display language and falls back to the
; first entry (English) otherwise. Keep this list in step with the set
; the application's build accepts.
[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "ca"; MessagesFile: "compiler:Languages\Catalan.isl"
Name: "de"; MessagesFile: "compiler:Languages\German.isl"
Name: "es"; MessagesFile: "compiler:Languages\Spanish.isl"
Name: "pt"; MessagesFile: "compiler:Languages\Portuguese.isl"
Name: "uk"; MessagesFile: "compiler:Languages\Ukrainian.isl"

; Translations for the strings this script defines itself, one per wizard
; language. The language prefixes match the [Languages] names above.
[CustomMessages]
en.AddCliToPath=Add the command-line tool to the PATH
ca.AddCliToPath=Afegeix l'eina de línia d'ordres al PATH
de.AddCliToPath=Das Befehlszeilenwerkzeug zum PATH hinzufügen
es.AddCliToPath=Añadir la herramienta de línea de comandos al PATH
pt.AddCliToPath=Adicionar a ferramenta de linha de comandos ao PATH
uk.AddCliToPath=Додати інструмент командного рядка до PATH

[Tasks]
Name: "addtopath"; Description: "{cm:AddCliToPath}"

[Files]
Source: "{#StageDir}\*"; DestDir: "{app}"; Flags: recursesubdirs createallsubdirs ignoreversion

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#GuiExe}"
Name: "{group}\Uninstall {#AppName}"; Filename: "{uninstallexe}"

[Registry]
; Append the install directory to the PATH so sc-slicer-cli is reachable
; from a shell. HKLM (system PATH) for a per-machine install, HKCU (user
; PATH) for a per-user install; the NeedsAddPath check keeps it idempotent.
Root: HKLM; Subkey: "SYSTEM\CurrentControlSet\Control\Session Manager\Environment"; \
    ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; \
    Flags: preservestringtype; Tasks: addtopath; \
    Check: IsAdminInstallMode and NeedsAddPath(ExpandConstant('{app}'))
Root: HKCU; Subkey: "Environment"; \
    ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; \
    Flags: preservestringtype; Tasks: addtopath; \
    Check: (not IsAdminInstallMode) and NeedsAddPath(ExpandConstant('{app}'))

[Code]
function PathRootKey(): Integer;
begin
  if IsAdminInstallMode then
    Result := HKEY_LOCAL_MACHINE
  else
    Result := HKEY_CURRENT_USER;
end;

function PathSubKey(): string;
begin
  if IsAdminInstallMode then
    Result := 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment'
  else
    Result := 'Environment';
end;

{ True when the directory is not already a PATH entry, so the [Registry]
  append runs at most once. }
function NeedsAddPath(Param: string): Boolean;
var
  OrigPath: string;
begin
  if not RegQueryStringValue(PathRootKey(), PathSubKey(), 'Path', OrigPath) then
  begin
    Result := True;
    exit;
  end;
  Result := Pos(';' + Uppercase(Param) + ';', ';' + Uppercase(OrigPath) + ';') = 0;
end;

{ Rebuild the PATH without the given directory, dropping empty segments. }
procedure RemoveFromPath(const Dir: string);
var
  OrigPath: string;
  NewPath: string;
  Item: string;
  P: Integer;
begin
  if not RegQueryStringValue(PathRootKey(), PathSubKey(), 'Path', OrigPath) then
    exit;
  NewPath := '';
  OrigPath := OrigPath + ';';
  while Length(OrigPath) > 0 do
  begin
    P := Pos(';', OrigPath);
    Item := Copy(OrigPath, 1, P - 1);
    Delete(OrigPath, 1, P);
    if (Item <> '') and (CompareText(Item, Dir) <> 0) then
    begin
      if NewPath <> '' then
        NewPath := NewPath + ';';
      NewPath := NewPath + Item;
    end;
  end;
  RegWriteExpandStringValue(PathRootKey(), PathSubKey(), 'Path', NewPath);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    RemoveFromPath(ExpandConstant('{app}'));
end;
