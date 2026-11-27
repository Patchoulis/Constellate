; Additions to the Windows installer electron-builder makes (NSIS).
;
; Uninstalling removes the program. What the app made while it was used lives apart from it, in
; %LOCALAPPDATA%\Constellate (and the window's own settings in %APPDATA%\Constellate): its own Python and packages (several GB), and the library of papers, embeddings and
; runs. The uninstaller asks whether those should go too; an update never asks and never removes them.
!macro customUnInstall
  ${ifNot} ${isUpdated}
    MessageBox MB_YESNO|MB_ICONQUESTION "Also remove everything Constellate downloaded and computed?$\r$\n$\r$\nThat is its Python packages (several GB) and your library: papers, embeddings and runs.$\r$\n$\r$\nChoose No to keep them for a later reinstall." /SD IDNO IDNO keepData
      RMDir /r "$LOCALAPPDATA\Constellate"
      RMDir /r "$APPDATA\Constellate"
    keepData:
  ${endIf}
!macroend
