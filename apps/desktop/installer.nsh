; Additions to the Windows installer electron-builder makes (NSIS).
;
; Uninstalling removes the program. What the app made while it was used lives apart from it, in
; %LOCALAPPDATA%\Constellate (and the window's own settings in %APPDATA%\Constellate): the embedding model it
; downloaded at its first run (about 1.2 GB), and the library of papers, embeddings and runs, with its .env of keys
; if one was made. The uninstaller asks whether those should go too; an update never asks and never removes them.
!macro customUnInstall
  ${ifNot} ${isUpdated}
    MessageBox MB_YESNO|MB_ICONQUESTION "Also remove everything Constellate downloaded and computed?$\r$\n$\r$\nThat is the embedding model (about 1.2 GB) and your library: papers, embeddings and runs.$\r$\n$\r$\nChoose No to keep them for a later reinstall." /SD IDNO IDNO keepData
      RMDir /r "$LOCALAPPDATA\Constellate"
      RMDir /r "$APPDATA\Constellate"
    keepData:
  ${endIf}
!macroend
