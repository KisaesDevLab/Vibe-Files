Vibe Files - portable Windows build
===================================

Install
  1. Unzip this folder anywhere you can write to (for example C:\Vibe Files).
  2. Keep vibe-files.exe and pdfium.dll together in the same folder.
  3. Run vibe-files.exe.

Vibe Files is portable: it writes nothing to the registry. Its catalog, settings,
caches and logs live in the "data" folder created beside the exe. To move it to
another PC, move the whole folder. To remove it, delete the folder.

Verify the download
  The .sha256.txt file published with this zip lists SHA-256 hashes for the zip,
  vibe-files.exe and pdfium.dll. In PowerShell:
    Get-FileHash -Algorithm SHA256 .\vibe-files.exe

Licenses
  LICENSE.txt           Vibe Files (PolyForm Internal Use 1.0.0)
  licenses\pdfium\      PDFium and its bundled third-party components
  licenses\fonts\       IBM Plex fonts (SIL Open Font License 1.1)
