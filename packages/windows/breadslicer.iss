; SPDX-FileCopyrightText: 2026 SynrgStudio
; SPDX-License-Identifier: AGPL-3.0-only
;
; The Breadslicer installer for Windows. `installer.py` builds it from the
; release folder `portable/package.py` writes (the executables, the Visual C++
; runtime beside them, the profiles, the licence and notices) and the
; Explorer thumbnail provider of .bread files.
;
; It installs for the person running it, with no administrator rights,
; unless they pick everyone; .bread opens with Breadslicer and shows its
; thumbnail in Explorer; STL, 3MF and OBJ get Breadslicer in "Open with"
; without taking them from the program that opens them now.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef VersionNumbers
  #define VersionNumbers "0.0.0.0"
#endif
#ifndef SourceDir
  #error Pass /DSourceDir=<the release folder>
#endif
#ifndef OutputDir
  #define OutputDir "."
#endif

; A dev build (`installer.py` passes /DDev for a version X.Y.Z-dev.N, #766)
; is an app of its own, Breadslicer Dev: its own id, folder, Start menu
; entry, file types and uninstaller, so it installs and updates beside the
; release. It takes .bread only when asked.
#ifdef Dev
  #define AppName "Breadslicer Dev"
  #define AppGuid "{{507E3DB7-48BE-4AD6-91B7-94FE0B2420A9}"
  #define ProgPrefix "BreadslicerDev"
#else
  #define AppName "Breadslicer"
  #define AppGuid "{{125F8ED7-70C9-43A4-8622-F4E435433124}"
  #define ProgPrefix "Breadslicer"
#endif
#define GuiExe "breadslicer.exe"
#define ProgIdBread ProgPrefix + ".bread"
#define ProgIdStl ProgPrefix + ".stl"
#define ProgId3mf ProgPrefix + ".3mf"
#define ProgIdObj ProgPrefix + ".obj"
; The thumbnail provider (bread-format's crate bread-thumbnails) and the
; shell's key for thumbnail handlers. `{{` is a literal `{`.
#define ThumbClsid "{{AF9572F8-199D-4710-88C0-5D553D38F093}"
#define ThumbHandler "{{E357FCCD-A995-4576-B01F-234630154E96}"

[Setup]
AppId={#AppGuid}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
VersionInfoVersion={#VersionNumbers}
AppPublisher=SynrgStudio
AppPublisherURL=https://breadslicer.app
AppSupportURL=https://github.com/breadslicer-app/breadslicer/issues
AppUpdatesURL=https://github.com/breadslicer-app/breadslicer/releases
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
OutputDir={#OutputDir}
OutputBaseFilename=Breadslicer-{#AppVersion}-windows-x64-setup
LicenseFile={#SourceDir}\LICENSE.md
SetupIconFile=..\..\apps\slicer-gui\assets\icons\breadslicer.ico
UninstallDisplayIcon={app}\{#GuiExe}
UninstallDisplayName={#AppName}
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog commandline
UsedUserAreasWarning=no
ChangesAssociations=yes
ChangesEnvironment=yes
CloseApplications=force
RestartApplications=no

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "es"; MessagesFile: "compiler:Languages\Spanish.isl"
Name: "pt"; MessagesFile: "compiler:Languages\BrazilianPortuguese.isl"
Name: "de"; MessagesFile: "compiler:Languages\German.isl"
Name: "fr"; MessagesFile: "compiler:Languages\French.isl"
Name: "it"; MessagesFile: "compiler:Languages\Italian.isl"

[CustomMessages]
en.AssocGroup=Files:
es.AssocGroup=Archivos:
pt.AssocGroup=Arquivos:
de.AssocGroup=Dateien:
fr.AssocGroup=Fichiers :
it.AssocGroup=File:
en.AssocBread=Open .bread files with {#AppName}
es.AssocBread=Abrir los archivos .bread con {#AppName}
pt.AssocBread=Abrir arquivos .bread com o {#AppName}
de.AssocBread=.bread-Dateien mit {#AppName} öffnen
fr.AssocBread=Ouvrir les fichiers .bread avec {#AppName}
it.AssocBread=Apri i file .bread con {#AppName}
en.OpenWith=Add {#AppName} to "Open with" for STL, 3MF and OBJ models
es.OpenWith=Agregar {#AppName} a "Abrir con" en los modelos STL, 3MF y OBJ
pt.OpenWith=Adicionar o {#AppName} a "Abrir com" para modelos STL, 3MF e OBJ
de.OpenWith={#AppName} unter "Öffnen mit" für STL-, 3MF- und OBJ-Modelle anbieten
fr.OpenWith=Ajouter {#AppName} à « Ouvrir avec » pour les modèles STL, 3MF et OBJ
it.OpenWith=Aggiungi {#AppName} ad "Apri con" per i modelli STL, 3MF e OBJ
en.AddCliToPath=Add the command-line tool to the PATH
es.AddCliToPath=Agregar la herramienta de línea de comandos al PATH
pt.AddCliToPath=Adicionar a ferramenta de linha de comando ao PATH
de.AddCliToPath=Das Befehlszeilenwerkzeug zum PATH hinzufügen
fr.AddCliToPath=Ajouter l'outil en ligne de commande au PATH
it.AddCliToPath=Aggiungi lo strumento da riga di comando al PATH
en.TypeBread=Bread 3D file
es.TypeBread=Archivo 3D Bread
pt.TypeBread=Arquivo 3D Bread
de.TypeBread=Bread-3D-Datei
fr.TypeBread=Fichier 3D Bread
it.TypeBread=File 3D Bread
en.TypeStl=STL 3D model
es.TypeStl=Modelo 3D STL
pt.TypeStl=Modelo 3D STL
de.TypeStl=STL-3D-Modell
fr.TypeStl=Modèle 3D STL
it.TypeStl=Modello 3D STL
en.Type3mf=3MF 3D model
es.Type3mf=Modelo 3D 3MF
pt.Type3mf=Modelo 3D 3MF
de.Type3mf=3MF-3D-Modell
fr.Type3mf=Modèle 3D 3MF
it.Type3mf=Modello 3D 3MF
en.TypeObj=OBJ 3D model
es.TypeObj=Modelo 3D OBJ
pt.TypeObj=Modelo 3D OBJ
de.TypeObj=OBJ-3D-Modell
fr.TypeObj=Modèle 3D OBJ
it.TypeObj=Modello 3D OBJ

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
#ifdef Dev
Name: "assocbread"; Description: "{cm:AssocBread}"; GroupDescription: "{cm:AssocGroup}"; Flags: unchecked
#else
Name: "assocbread"; Description: "{cm:AssocBread}"; GroupDescription: "{cm:AssocGroup}"
#endif
Name: "openwith"; Description: "{cm:OpenWith}"; GroupDescription: "{cm:AssocGroup}"
Name: "addtopath"; Description: "{cm:AddCliToPath}"; Flags: unchecked

[Files]
; The thumbnail provider can be loaded by Explorer: replace it at the next
; start when it is, and take it off then on uninstall.
Source: "{#SourceDir}\bread_thumbnails.dll"; DestDir: "{app}"; \
    Flags: ignoreversion restartreplace uninsrestartdelete
Source: "{#SourceDir}\*"; DestDir: "{app}"; Excludes: "bread_thumbnails.dll,SHA256SUMS.txt"; \
    Flags: recursesubdirs createallsubdirs ignoreversion

[InstallDelete]
; The bundled profiles are replaced whole: a profile a release took out
; must not stay behind. The person's own profiles live in their data folder.
Type: filesandordirs; Name: "{app}\slicer\profiles"

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#GuiExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#GuiExe}"; Tasks: desktopicon

[Registry]
; .bread: opens with Breadslicer, when asked. Uninstalling takes it off only
; while it is still Breadslicer (see [Code]).
Root: HKA; Subkey: "Software\Classes\.bread"; ValueType: string; ValueName: ""; \
    ValueData: "{#ProgIdBread}"; Tasks: assocbread
Root: HKA; Subkey: "Software\Classes\.bread\OpenWithProgids"; ValueType: string; \
    ValueName: "{#ProgIdBread}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\{#ProgIdBread}"; ValueType: string; ValueName: ""; \
    ValueData: "{cm:TypeBread}"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\{#ProgIdBread}\DefaultIcon"; ValueType: string; ValueName: ""; \
    ValueData: "{app}\{#GuiExe},0"
Root: HKA; Subkey: "Software\Classes\{#ProgIdBread}\shell\open\command"; ValueType: string; ValueName: ""; \
    ValueData: """{app}\{#GuiExe}"" ""%1"""

; STL, 3MF, OBJ: in "Open with", the program that opens them now kept.
Root: HKA; Subkey: "Software\Classes\{#ProgIdStl}"; ValueType: string; ValueName: ""; \
    ValueData: "{cm:TypeStl}"; Flags: uninsdeletekey; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\{#ProgIdStl}\DefaultIcon"; ValueType: string; ValueName: ""; \
    ValueData: "{app}\{#GuiExe},0"; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\{#ProgIdStl}\shell\open\command"; ValueType: string; ValueName: ""; \
    ValueData: """{app}\{#GuiExe}"" ""%1"""; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\.stl\OpenWithProgids"; ValueType: string; \
    ValueName: "{#ProgIdStl}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\{#ProgId3mf}"; ValueType: string; ValueName: ""; \
    ValueData: "{cm:Type3mf}"; Flags: uninsdeletekey; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\{#ProgId3mf}\DefaultIcon"; ValueType: string; ValueName: ""; \
    ValueData: "{app}\{#GuiExe},0"; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\{#ProgId3mf}\shell\open\command"; ValueType: string; ValueName: ""; \
    ValueData: """{app}\{#GuiExe}"" ""%1"""; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\.3mf\OpenWithProgids"; ValueType: string; \
    ValueName: "{#ProgId3mf}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\{#ProgIdObj}"; ValueType: string; ValueName: ""; \
    ValueData: "{cm:TypeObj}"; Flags: uninsdeletekey; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\{#ProgIdObj}\DefaultIcon"; ValueType: string; ValueName: ""; \
    ValueData: "{app}\{#GuiExe},0"; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\{#ProgIdObj}\shell\open\command"; ValueType: string; ValueName: ""; \
    ValueData: """{app}\{#GuiExe}"" ""%1"""; Tasks: openwith
Root: HKA; Subkey: "Software\Classes\.obj\OpenWithProgids"; ValueType: string; \
    ValueName: "{#ProgIdObj}"; ValueData: ""; Flags: uninsdeletevalue; Tasks: openwith

; Explorer thumbnails of .bread files (bread-format's provider), whichever
; program opens them: on the .bread key, not on the ProgID. Not through the
; DLL's own registration, which writes for the current user alone. Other
; programs ship the same provider: one already registered and in place is
; left as it is, and uninstalling takes these keys off only while they point
; here (see [Code]).
Root: HKA; Subkey: "Software\Classes\CLSID\{#ThumbClsid}"; ValueType: string; ValueName: ""; \
    ValueData: "Bread thumbnail provider"; Check: ThumbnailsAreOurs
Root: HKA; Subkey: "Software\Classes\CLSID\{#ThumbClsid}\InprocServer32"; ValueType: string; ValueName: ""; \
    ValueData: "{app}\bread_thumbnails.dll"; Check: ThumbnailsAreOurs
Root: HKA; Subkey: "Software\Classes\CLSID\{#ThumbClsid}\InprocServer32"; ValueType: string; \
    ValueName: "ThreadingModel"; ValueData: "Apartment"; Check: ThumbnailsAreOurs
Root: HKA; Subkey: "Software\Classes\.bread\ShellEx\{#ThumbHandler}"; ValueType: string; ValueName: ""; \
    ValueData: "{#ThumbClsid}"; Check: ThumbnailsAreOurs

; The command-line tool on the PATH, when asked.
Root: HKCU; Subkey: "Environment"; ValueType: expandsz; ValueName: "Path"; \
    ValueData: "{olddata};{app}"; Flags: preservestringtype; Tasks: addtopath; \
    Check: (not IsAdminInstallMode) and NeedsAddPath(ExpandConstant('{app}'))
Root: HKLM; Subkey: "SYSTEM\CurrentControlSet\Control\Session Manager\Environment"; \
    ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; \
    Flags: preservestringtype; Tasks: addtopath; \
    Check: IsAdminInstallMode and NeedsAddPath(ExpandConstant('{app}'))

[Run]
Filename: "{app}\{#GuiExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; \
    Flags: nowait postinstall skipifsilent

[Code]
// Whether `Dir` is not yet one of the PATH's entries.
function NeedsAddPath(Dir: string): Boolean;
var
  Path: string;
  RootKey: Integer;
  SubKey: string;
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
  if not RegQueryStringValue(RootKey, SubKey, 'Path', Path) then
  begin
    Result := True;
    exit;
  end;
  Result := Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(Path) + ';') = 0;
end;

// Takes `Dir` off the PATH on uninstall, wherever it was put.
procedure RemoveFromPath(RootKey: Integer; SubKey: string; Dir: string);
var
  Path: string;
  P: Integer;
begin
  if not RegQueryStringValue(RootKey, SubKey, 'Path', Path) then
    exit;
  P := Pos(';' + Uppercase(Dir), ';' + Uppercase(Path));
  if P = 0 then
    exit;
  // P is the place of the entry in Path, counting the ';' added in front.
  if P = 1 then
    Delete(Path, 1, Length(Dir) + 1)
  else
    Delete(Path, P - 1, Length(Dir) + 1);
  RegWriteExpandStringValue(RootKey, SubKey, 'Path', Path);
end;

// The registry root the install wrote its classes to.
function ClassesRoot(): Integer;
begin
  if IsAdminInstallMode then
    Result := HKEY_LOCAL_MACHINE
  else
    Result := HKEY_CURRENT_USER;
end;

const
  ThumbKey = 'Software\Classes\CLSID\{AF9572F8-199D-4710-88C0-5D553D38F093}';
  ThumbShellKey = 'Software\Classes\.bread\ShellEx\{E357FCCD-A995-4576-B01F-234630154E96}';

// Whether the thumbnail provider is Breadslicer's to register: no other one
// registered whose file is still in place.
function ThumbnailsAreOurs(): Boolean;
var
  Dll: string;
begin
  if not RegQueryStringValue(ClassesRoot(), ThumbKey + '\InprocServer32', '', Dll) then
    Result := True
  else
    Result := (CompareText(Dll, ExpandConstant('{app}\bread_thumbnails.dll')) = 0) or not FileExists(Dll);
end;

// Takes off what of .bread is still Breadslicer's: the program that opens
// it, and the thumbnail provider while it is the one installed here. What
// another program put there since stays.
procedure ForgetBread();
var
  Value: string;
begin
  if RegQueryStringValue(ClassesRoot(), 'Software\Classes\.bread', '', Value) and
     (Value = '{#ProgIdBread}') then
    RegDeleteValue(ClassesRoot(), 'Software\Classes\.bread', '');
  if RegQueryStringValue(ClassesRoot(), ThumbKey + '\InprocServer32', '', Value) and
     (CompareText(Value, ExpandConstant('{app}\bread_thumbnails.dll')) = 0) then
  begin
    RegDeleteKeyIncludingSubkeys(ClassesRoot(), ThumbKey);
    RegDeleteKeyIncludingSubkeys(ClassesRoot(), ThumbShellKey);
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    ForgetBread();
  if CurUninstallStep = usPostUninstall then
  begin
    RemoveFromPath(HKEY_CURRENT_USER, 'Environment', ExpandConstant('{app}'));
    if IsAdminInstallMode then
      RemoveFromPath(HKEY_LOCAL_MACHINE,
        'SYSTEM\CurrentControlSet\Control\Session Manager\Environment',
        ExpandConstant('{app}'));
  end;
end;
