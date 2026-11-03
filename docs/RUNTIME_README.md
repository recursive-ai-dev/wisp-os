# WISP OS runtime

This package boots directly on an x86_64 UEFI machine. It includes the EFI
executable and bundled WISP programs; no compiler or underlying operating
system is required to run it. The precise version and binary SHA-256 are in
`BUILD_INFO.json`.

Verify the extracted files with `sha256sum -c SHA256SUMS`. Copy the contents
of `esp/` onto a FAT32 boot USB so the executable is at `EFI/BOOT/BOOTX64.EFI`,
then select that USB in the machine's firmware boot menu. The executable is
unsigned; firmware Secure Boot must trust your own signature or be configured
to allow it. See [USB_INSTALL.md](USB_INSTALL.md) for the included Linux,
macOS and Windows installers and their disk-erasure confirmations.

At the shell, `help` lists commands, `ls APPS` lists examples, and `run hello`
starts a program. `ide` opens Wisp Studio; F2 saves, F5 runs, F6 checks syntax,
and F10 exits. Esc stops a running program. A supported save disk is needed
for persistence; `disks` lists candidates and `disk <n>` selects one. Formatting
a save region requires an explicit confirmation.

See the [manual](docs/MANUAL.md), [language reference](docs/WISP_LANGUAGE.md)
and [release notes](docs/RELEASE_NOTES.md). Source archives, verification
evidence and checksums are available from the
[GitHub releases](https://github.com/recursive-ai-dev/wisp-os/releases).
