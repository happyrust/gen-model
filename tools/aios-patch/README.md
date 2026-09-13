# AIOS binary patch

The standalone GPUI `AIOS-Patch-Maker.exe` is the quickest path:

- Double-click it, select the exact installed `aios-database.exe`, select the
  new `aios-database.exe`, review both version/build/hash identities, and click
  **Generate Patch**; or
- run `AIOS-Patch-Maker.exe make <old-exe-or-zip> <new-exe-or-zip>`.

The GPUI window keeps file inspection and patch generation off the UI thread,
shows success or failure in place, and can reveal the generated file in
Explorer. The maker contains the patch engine; no companion engine executable
is required. The default GUI flow writes the patch beside the old EXE. The
command-line `make` flow additionally accepts EXE + ZIP, ZIP + EXE, and ZIP +
ZIP.

For ZIP input it extracts only the package-root `aios-database.exe` into a
temporary directory. It reads both build IDs itself, chooses a versioned output
name, generates the self-contained patch, and verifies dry-run, real
replacement, backup identity, and second-run idempotence before reporting
success. The default output directory is beside the old EXE/ZIP.

Generate a self-contained patch:

```powershell
.\scripts\New-AiosDatabasePatch.ps1 `
  -PreviousExe D:\release\old\aios-database.exe `
  -NewExe D:\release\new\aios-database.exe
```

Put the generated `*.patch.exe` beside the old `aios-database.exe`, stop the
service, then run the patch EXE. It verifies the exact source SHA-256, builds
and verifies the new EXE in a same-directory temporary file, and atomically
replaces the target while retaining `aios-database.exe.bak-<hash>`. A
double-clicked patch reports success or failure in a Windows message box.

The generator also emits `<patch>.install.ps1`. Run that companion script for
the operational path: checksum + dry-run, exact-process stop, patch, target
hash verification, service start, `/api/v1/health` build-id verification, and
automatic atomic rollback if startup fails.

The generator also verifies every deliverable in an isolated directory before
returning it: dry-run leaves the source untouched, no-argument execution
produces the exact target and one exact backup, and a second run reports
`already-current` without another backup.

Useful commands:

```powershell
.\update.patch.exe inspect
.\update.patch.exe apply --dry-run
.\update.patch.exe apply --target D:\AMS\aios-database.exe
.\update.patch.exe.install.ps1
```

The executable is integrity-checked but not cryptographically signed. Sign the
final generated patch EXE with the normal release certificate before external
distribution.
