; Inno Setup script for the Breadslicer Windows installer.
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

#define AppName "Breadslicer"
#define GuiExe "breadslicer.exe"
#define CliExe "sc-slicer-cli.exe"

; One ProgID per format the GUI can open. Each is a key this installer
; owns outright, so uninstalling removes exactly what was added and
; nothing that belonged to another application.
#define ProgIdStl "SoulCrafted.Slicer.stl"
#define ProgIdObj "SoulCrafted.Slicer.obj"
#define ProgId3mf "SoulCrafted.Slicer.3mf"
#define ProgIdBread "SoulCrafted.Slicer.bread"

[Setup]
AppId=it.soulcrafted.slicer
AppName={#AppName}
AppVersion={#AppVersion}
VersionInfoVersion={#AppVersion}
AppPublisher=SynrgStudio
AppPublisherURL=https://breadslicer.app
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
OutputDir={#OutputDir}
OutputBaseFilename=sc-slicer-setup-{#AppVersion}-x86_64
LicenseFile=..\..\LICENSE.md
SetupIconFile=..\..\apps\slicer-gui\assets\icons\it.soulcrafted.slicer.ico
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
; The file associations below change the shell's view of what opens a
; model file; this makes Setup notify Explorer so icons and the
; double-click handler update without a sign-out.
ChangesAssociations=yes

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

en.FileAssociations=Open these files with {#AppName}:
ca.FileAssociations=Obre aquests fitxers amb {#AppName}:
de.FileAssociations=Diese Dateien mit {#AppName} öffnen:
es.FileAssociations=Abrir estos archivos con {#AppName}:
pt.FileAssociations=Abrir estes ficheiros com o {#AppName}:
uk.FileAssociations=Відкривати ці файли за допомогою {#AppName}:

en.AssocBread=Breadslicer projects (.bread)
ca.AssocBread=Projectes de Breadslicer (.bread)
de.AssocBread=Breadslicer-Projekte (.bread)
es.AssocBread=Proyectos de Breadslicer (.bread)
pt.AssocBread=Projetos do Breadslicer (.bread)
uk.AssocBread=Проєкти Breadslicer (.bread)

en.AssocStl=STL models (.stl, .stla, .stlb)
ca.AssocStl=Models STL (.stl, .stla, .stlb)
de.AssocStl=STL-Modelle (.stl, .stla, .stlb)
es.AssocStl=Modelos STL (.stl, .stla, .stlb)
pt.AssocStl=Modelos STL (.stl, .stla, .stlb)
uk.AssocStl=Моделі STL (.stl, .stla, .stlb)

en.Assoc3mf=3MF models (.3mf)
ca.Assoc3mf=Models 3MF (.3mf)
de.Assoc3mf=3MF-Modelle (.3mf)
es.Assoc3mf=Modelos 3MF (.3mf)
pt.Assoc3mf=Modelos 3MF (.3mf)
uk.Assoc3mf=Моделі 3MF (.3mf)

en.AssocObj=Wavefront OBJ models (.obj)
ca.AssocObj=Models Wavefront OBJ (.obj)
de.AssocObj=Wavefront-OBJ-Modelle (.obj)
es.AssocObj=Modelos Wavefront OBJ (.obj)
pt.AssocObj=Modelos Wavefront OBJ (.obj)
uk.AssocObj=Моделі Wavefront OBJ (.obj)

; Shown in Explorer's Type column. Written once, in the language the
; installer ran in, which is the same trade-off every installer makes for
; a registry-resident string.
en.TypeBread=Breadslicer Project
ca.TypeBread=Projecte de Breadslicer
de.TypeBread=Breadslicer-Projekt
es.TypeBread=Proyecto de Breadslicer
pt.TypeBread=Projeto do Breadslicer
uk.TypeBread=Проєкт Breadslicer

en.TypeStl=STL 3D Model
ca.TypeStl=Model 3D STL
de.TypeStl=STL-3D-Modell
es.TypeStl=Modelo 3D STL
pt.TypeStl=Modelo 3D STL
uk.TypeStl=3D-модель STL

en.Type3mf=3MF 3D Model
ca.Type3mf=Model 3D 3MF
de.Type3mf=3MF-3D-Modell
es.Type3mf=Modelo 3D 3MF
pt.Type3mf=Modelo 3D 3MF
uk.Type3mf=3D-модель 3MF

en.TypeObj=Wavefront OBJ 3D Model
ca.TypeObj=Model 3D Wavefront OBJ
de.TypeObj=Wavefront-OBJ-3D-Modell
es.TypeObj=Modelo 3D Wavefront OBJ
pt.TypeObj=Modelo 3D Wavefront OBJ
uk.TypeObj=3D-модель Wavefront OBJ

[Tasks]
Name: "addtopath"; Description: "{cm:AddCliToPath}"

; The formats a resin slicer is handed most often are checked by default;
; OBJ is a general-purpose format more likely to already belong to another
; application, so claiming it is opt-in. None of these overwrite an
; existing association: see ExtensionIsUnclaimed below.
; .bread is Breadslicer's own project format, so it is checked by default.
Name: "assocbread"; Description: "{cm:AssocBread}"; GroupDescription: "{cm:FileAssociations}"
Name: "assocstl"; Description: "{cm:AssocStl}"; GroupDescription: "{cm:FileAssociations}"
Name: "assoc3mf"; Description: "{cm:Assoc3mf}"; GroupDescription: "{cm:FileAssociations}"
Name: "assocobj"; Description: "{cm:AssocObj}"; GroupDescription: "{cm:FileAssociations}"; Flags: unchecked

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

; File associations. HKA resolves to HKLM for a per-machine install and
; HKCU for a per-user one, so the associations land in the same scope the
; user chose for the application itself.
;
; Each format is registered in two steps. The ProgID describes how to open
; the format and is deleted wholesale on uninstall. The extension's
; OpenWithProgids entry adds the application to Explorer's "Open with"
; list, which is additive: it makes the application available without
; taking the format away from whatever already handles it.
Root: HKA; Subkey: "Software\Classes\{#ProgIdBread}"; ValueType: string; ValueName: ""; \
    ValueData: "{cm:TypeBread}"; Flags: uninsdeletekey; Tasks: assocbread
Root: HKA; Subkey: "Software\Classes\{#ProgIdBread}\DefaultIcon"; ValueType: string; ValueName: ""; \
    ValueData: "{app}\{#GuiExe},0"; Tasks: assocbread
Root: HKA; Subkey: "Software\Classes\{#ProgIdBread}\shell\open\command"; ValueType: string; ValueName: ""; \
    ValueData: """{app}\{#GuiExe}"" ""%1"""; Tasks: assocbread

Root: HKA; Subkey: "Software\Classes\{#ProgIdStl}"; ValueType: string; ValueName: ""; \
    ValueData: "{cm:TypeStl}"; Flags: uninsdeletekey; Tasks: assocstl
Root: HKA; Subkey: "Software\Classes\{#ProgIdStl}\DefaultIcon"; ValueType: string; ValueName: ""; \
    ValueData: "{app}\{#GuiExe},0"; Tasks: assocstl
Root: HKA; Subkey: "Software\Classes\{#ProgIdStl}\shell\open\command"; ValueType: string; ValueName: ""; \
    ValueData: """{app}\{#GuiExe}"" ""%1"""; Tasks: assocstl

Root: HKA; Subkey: "Software\Classes\{#ProgId3mf}"; ValueType: string; ValueName: ""; \
    ValueData: "{cm:Type3mf}"; Flags: uninsdeletekey; Tasks: assoc3mf
Root: HKA; Subkey: "Software\Classes\{#ProgId3mf}\DefaultIcon"; ValueType: string; ValueName: ""; \
    ValueData: "{app}\{#GuiExe},0"; Tasks: assoc3mf
Root: HKA; Subkey: "Software\Classes\{#ProgId3mf}\shell\open\command"; ValueType: string; ValueName: ""; \
    ValueData: """{app}\{#GuiExe}"" ""%1"""; Tasks: assoc3mf

Root: HKA; Subkey: "Software\Classes\{#ProgIdObj}"; ValueType: string; ValueName: ""; \
    ValueData: "{cm:TypeObj}"; Flags: uninsdeletekey; Tasks: assocobj
Root: HKA; Subkey: "Software\Classes\{#ProgIdObj}\DefaultIcon"; ValueType: string; ValueName: ""; \
    ValueData: "{app}\{#GuiExe},0"; Tasks: assocobj
Root: HKA; Subkey: "Software\Classes\{#ProgIdObj}\shell\open\command"; ValueType: string; ValueName: ""; \
    ValueData: """{app}\{#GuiExe}"" ""%1"""; Tasks: assocobj

; Offer the application for each extension. The .stla and .stlb variants
; are the ASCII/binary-specific names the loader accepts, and ride along
; with the STL checkbox rather than getting boxes of their own.
Root: HKA; Subkey: "Software\Classes\.bread\OpenWithProgids"; ValueType: string; \
    ValueName: "{#ProgIdBread}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assocbread
Root: HKA; Subkey: "Software\Classes\.stl\OpenWithProgids"; ValueType: string; \
    ValueName: "{#ProgIdStl}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assocstl
Root: HKA; Subkey: "Software\Classes\.stla\OpenWithProgids"; ValueType: string; \
    ValueName: "{#ProgIdStl}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assocstl
Root: HKA; Subkey: "Software\Classes\.stlb\OpenWithProgids"; ValueType: string; \
    ValueName: "{#ProgIdStl}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assocstl
Root: HKA; Subkey: "Software\Classes\.3mf\OpenWithProgids"; ValueType: string; \
    ValueName: "{#ProgId3mf}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc3mf
Root: HKA; Subkey: "Software\Classes\.obj\OpenWithProgids"; ValueType: string; \
    ValueName: "{#ProgIdObj}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assocobj

; Become the default handler only for extensions nobody has claimed. On a
; machine where another slicer or CAD tool already owns the format, ticking
; the box adds this application to "Open with" and stops there; silently
; taking over a format the user already assigned elsewhere is not the
; installer's call to make. The uninstaller reverses this only where the
; entry still points at us.
Root: HKA; Subkey: "Software\Classes\.bread"; ValueType: string; ValueName: ""; \
    ValueData: "{#ProgIdBread}"; Tasks: assocbread; Check: ExtensionIsUnclaimed('.bread')
Root: HKA; Subkey: "Software\Classes\.stl"; ValueType: string; ValueName: ""; \
    ValueData: "{#ProgIdStl}"; Tasks: assocstl; Check: ExtensionIsUnclaimed('.stl')
Root: HKA; Subkey: "Software\Classes\.stla"; ValueType: string; ValueName: ""; \
    ValueData: "{#ProgIdStl}"; Tasks: assocstl; Check: ExtensionIsUnclaimed('.stla')
Root: HKA; Subkey: "Software\Classes\.stlb"; ValueType: string; ValueName: ""; \
    ValueData: "{#ProgIdStl}"; Tasks: assocstl; Check: ExtensionIsUnclaimed('.stlb')
Root: HKA; Subkey: "Software\Classes\.3mf"; ValueType: string; ValueName: ""; \
    ValueData: "{#ProgId3mf}"; Tasks: assoc3mf; Check: ExtensionIsUnclaimed('.3mf')
Root: HKA; Subkey: "Software\Classes\.obj"; ValueType: string; ValueName: ""; \
    ValueData: "{#ProgIdObj}"; Tasks: assocobj; Check: ExtensionIsUnclaimed('.obj')

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

{ The registry hive the associations live in, matching the HKA used by the
  entries above: machine-wide for an elevated install, per-user otherwise.
  A line here may not begin with a bracketed word, because the script
  parser reads one as a section header even inside a comment. }
function ClassesRootKey(): Integer;
begin
  if IsAdminInstallMode then
    Result := HKEY_LOCAL_MACHINE
  else
    Result := HKEY_CURRENT_USER;
end;

{ True when no application currently opens this extension, so claiming it
  as the default takes nothing away from anyone.

  Three places have to agree before that is true. A per-user association in
  HKCU shadows a machine-wide one, so both hives are consulted regardless
  of which one this install writes to. UserChoice is checked last because
  on Windows 8 and later it outranks both: when the user has picked a
  handler themselves, that choice stands and Setup leaves the extension
  alone. }
function ExtensionIsUnclaimed(Param: string): Boolean;
var
  Existing: string;
begin
  Result := False;
  if RegQueryStringValue(HKEY_CURRENT_USER, 'Software\Classes\' + Param, '', Existing) and
     (Existing <> '') then
    exit;
  if RegQueryStringValue(HKEY_LOCAL_MACHINE, 'Software\Classes\' + Param, '', Existing) and
     (Existing <> '') then
    exit;
  if RegQueryStringValue(HKEY_CURRENT_USER,
       'Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\' + Param + '\UserChoice',
       'ProgId', Existing) and (Existing <> '') then
    exit;
  Result := True;
end;

{ Give back an extension this install claimed, but only while it still
  points at our ProgID. If another application has taken the format over in
  the meantime, that is the association the user wants now and removing it
  would leave the extension orphaned. }
procedure UnclaimExtension(const Ext, ProgId: string);
var
  Current: string;
begin
  if RegQueryStringValue(ClassesRootKey(), 'Software\Classes\' + Ext, '', Current) and
     (CompareText(Current, ProgId) = 0) then
    RegDeleteValue(ClassesRootKey(), 'Software\Classes\' + Ext, '');
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
  begin
    RemoveFromPath(ExpandConstant('{app}'));
    { The ProgID keys and the OpenWithProgids entries are removed by the
      uninsdeletekey / uninsdeletevalue flags. Only the default-handler
      values set conditionally at install time need deciding at run time. }
    UnclaimExtension('.bread', '{#ProgIdBread}');
    UnclaimExtension('.stl', '{#ProgIdStl}');
    UnclaimExtension('.stla', '{#ProgIdStl}');
    UnclaimExtension('.stlb', '{#ProgIdStl}');
    UnclaimExtension('.3mf', '{#ProgId3mf}');
    UnclaimExtension('.obj', '{#ProgIdObj}');
  end;
end;
