#Requires -RunAsAdministrator
<#
.SYNOPSIS
  Write the WISP UEFI shell to a USB flash drive on Windows, as a bootable
  GPT disk with a properly-typed EFI System Partition.

.DESCRIPTION
  Safety model (same intent as the Linux/macOS installers in this
  directory, adapted to Windows' Storage cmdlets):

    1. -List is entirely read-only and only shows disks Windows reports as
       BusType 'USB' -- it never touches a disk.
    2. -Install re-checks the target disk is BusType 'USB' and refuses
       otherwise unless -Force is passed.
    3. -Install unconditionally (no override) refuses the disk backing the
       Windows system/boot volume, so this script can never wipe the
       machine it's running on.
    4. -Install requires two separate typed confirmations: the exact disk
       number, and the literal word ERASE.

  This script must run in an elevated ("Run as Administrator") PowerShell
  window -- disk partitioning requires it.

.PARAMETER List
  Read-only: list candidate USB disks and exit.

.PARAMETER Install
  Switch that selects install mode. Requires -DiskNumber.

.PARAMETER DiskNumber
  The Windows disk number to install to (see -List). Required with
  -Install.

.PARAMETER Source
  Path to the ESP directory to copy from. Defaults to ..\esp relative to
  this script (i.e. run build.sh's output, produced under WSL or copied
  over, before using this script).

.PARAMETER Force
  Allow a disk that Windows doesn't report as BusType 'USB'. Does NOT
  bypass the system-disk safety check.

.EXAMPLE
  .\install_usb_windows.ps1 -List

.EXAMPLE
  .\install_usb_windows.ps1 -Install -DiskNumber 2

.NOTES
  Secure Boot: BOOTX64.EFI produced by this project is NOT signed. Real
  hardware with Secure Boot enabled will refuse to run it. Disable Secure
  Boot in the target machine's firmware setup before booting this USB
  stick, or enroll your own signature -- this script does not attempt to
  do either for you.
#>
[CmdletBinding(DefaultParameterSetName = 'List')]
param(
    [Parameter(ParameterSetName = 'List')]
    [switch]$List,

    [Parameter(ParameterSetName = 'Install', Mandatory = $true)]
    [switch]$Install,

    [Parameter(ParameterSetName = 'Install', Mandatory = $true)]
    [int]$DiskNumber,

    [Parameter(ParameterSetName = 'Install')]
    [string]$Source,

    [Parameter(ParameterSetName = 'Install')]
    [switch]$Force
)

$ErrorActionPreference = 'Stop'

# The official EFI System Partition type GUID, per the UEFI spec. Setting
# this explicitly (rather than letting a generic tool guess) is what makes
# this partition unambiguously recognized as an ESP by firmware and by
# Windows itself, rather than relying on the "removable media" fallback
# path some other platforms lean on.
$EspTypeGuid = '{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}'

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
if (-not $Source) {
    $Source = Join-Path $ScriptDir '..\esp'
}

function Get-CandidateDisks {
    Get-Disk | Where-Object { $_.BusType -eq 'USB' }
}

function Get-SystemDiskNumber {
    # The disk number backing the currently running Windows installation,
    # identified via the system drive letter -> partition -> disk chain.
    # This is the check that stops you from wiping the machine you're
    # running the script on, so it deliberately has no -Force bypass.
    $sysDrive = $env:SystemDrive.TrimEnd(':')
    try {
        $part = Get-Partition -DriveLetter $sysDrive -ErrorAction Stop
        return $part.DiskNumber
    } catch {
        # If this lookup fails for any reason, fail closed: return -1 so
        # no real disk number can accidentally match it below is wrong --
        # instead, surface the failure so the operator investigates rather
        # than silently skipping the check.
        throw "Could not determine the system disk to protect it from being erased: $_"
    }
}

function Show-DiskList {
    $disks = Get-CandidateDisks
    Write-Host "==> Candidate disks (BusType = USB):"
    Write-Host ""
    if (-not $disks) {
        Write-Host "  (none found -- plug in your USB stick and try again)"
    } else {
        $disks | Format-Table -Property Number, FriendlyName,
            @{Label = 'Size(GB)'; Expression = { [math]::Round($_.Size / 1GB, 2) }},
            PartitionStyle, BusType -AutoSize | Out-Host
    }
    Write-Host ""
    Write-Host "Identify your USB stick above, then run:"
    Write-Host "  .\install_usb_windows.ps1 -Install -DiskNumber N"
}

function Install-ToDisk {
    param([int]$DiskNumber, [string]$Source, [bool]$Force)

    $sourceEfi = Join-Path $Source 'EFI\BOOT\BOOTX64.EFI'
    if (-not (Test-Path $sourceEfi)) {
        throw "Source ESP not found at '$Source' (expected EFI\BOOT\BOOTX64.EFI there). " +
              "Build the project first, or pass -Source."
    }

    $disk = Get-Disk -Number $DiskNumber -ErrorAction SilentlyContinue
    if (-not $disk) {
        throw "No disk with number $DiskNumber found. Run -List to see candidates."
    }

    # --- Safety check #1: never touch the system disk. No override. ---
    $systemDiskNumber = Get-SystemDiskNumber
    if ($DiskNumber -eq $systemDiskNumber) {
        throw "Disk $DiskNumber is the system disk this machine is running from. Refusing to touch it."
    }
    if ($disk.IsBoot -or $disk.IsSystem) {
        throw "Disk $DiskNumber is flagged as a boot/system disk by Windows. Refusing to touch it."
    }

    # --- Safety check #2: USB transport only, unless -Force. ---
    if ($disk.BusType -ne 'USB' -and -not $Force) {
        throw "Disk $DiskNumber has BusType '$($disk.BusType)', not USB. If you're certain this is " +
              "the right device, re-run with -Force."
    }
    if ($disk.BusType -ne 'USB') {
        Write-Warning "Proceeding on a non-USB-transport disk because -Force was given."
    }

    $sizeGb = [math]::Round($disk.Size / 1GB, 2)
    Write-Host "=================================================================="
    Write-Host " About to COMPLETELY ERASE and reformat:"
    Write-Host "   Disk number: $DiskNumber"
    Write-Host "   Model:       $($disk.FriendlyName)"
    Write-Host "   Size:        $sizeGb GB"
    Write-Host "   Bus type:    $($disk.BusType)"
    Write-Host "   Source:      $Source"
    Write-Host "=================================================================="
    Write-Host ""
    Write-Host "This will destroy ALL data currently on disk $DiskNumber."
    Write-Host ""

    $confirm1 = Read-Host "Type the disk number exactly ($DiskNumber) to continue"
    if ($confirm1 -ne "$DiskNumber") {
        throw "Confirmation did not match '$DiskNumber'. Aborting, nothing was touched."
    }
    $confirm2 = Read-Host "Type ERASE (all caps) to confirm you want to wipe this disk"
    if ($confirm2 -cne 'ERASE') {
        throw "Confirmation was not 'ERASE'. Aborting, nothing was touched."
    }

    Write-Host "==> Wiping existing partition table on disk $DiskNumber..."
    Clear-Disk -Number $DiskNumber -RemoveData -RemoveOEM -Confirm:$false

    Write-Host "==> Initializing disk $DiskNumber as GPT..."
    Initialize-Disk -Number $DiskNumber -PartitionStyle GPT

    Write-Host "==> Creating EFI System Partition (type $EspTypeGuid)..."
    # -AssignDriveLetter is requested up front (Microsoft's own documented
    # pattern for this exact New-Partition | Format-Volume flow), but some
    # Windows configurations decline to auto-assign a letter to a
    # GPT-ESP-typed partition even when asked, so this is re-checked and
    # backfilled explicitly below rather than trusted blindly.
    $partition = New-Partition -DiskNumber $DiskNumber -UseMaximumSize -GptType $EspTypeGuid -AssignDriveLetter

    Write-Host "==> Formatting as FAT32 (label WISPUSB)..."
    # Format-Volume takes a Volume/Partition via the pipeline, not a
    # -Partition parameter -- this is the documented New-Partition |
    # Format-Volume idiom.
    $partition | Format-Volume -FileSystem FAT32 -NewFileSystemLabel 'WISPUSB' -Confirm:$false | Out-Null

    $driveLetter = (Get-Partition -DiskNumber $DiskNumber -PartitionNumber $partition.PartitionNumber).DriveLetter
    if (-not $driveLetter) {
        Get-Partition -DiskNumber $DiskNumber -PartitionNumber $partition.PartitionNumber |
            Add-PartitionAccessPath -AssignDriveLetter
        $driveLetter = (Get-Partition -DiskNumber $DiskNumber -PartitionNumber $partition.PartitionNumber).DriveLetter
    }
    if (-not $driveLetter) {
        throw "Could not obtain a drive letter for the new partition on disk $DiskNumber."
    }
    $root = "${driveLetter}:\"

    Write-Host "==> Copying firmware and apps to $root..."
    New-Item -ItemType Directory -Force -Path (Join-Path $root 'EFI\BOOT') | Out-Null
    New-Item -ItemType Directory -Force -Path (Join-Path $root 'APPS') | Out-Null
    Copy-Item -Path $sourceEfi -Destination (Join-Path $root 'EFI\BOOT\BOOTX64.EFI') -Force
    $wispFiles = Join-Path $Source 'APPS\*.wisp'
    if (Test-Path $wispFiles) {
        Copy-Item -Path $wispFiles -Destination (Join-Path $root 'APPS') -Force
    }

    # Best-effort eject: the real work (partition, format, copy) is already
    # done and confirmed by this point, so a failure here (e.g. no Shell
    # COM object available, or the drive is momentarily busy) should be a
    # warning, not a script failure that makes a successful install look
    # like it errored out.
    Write-Host "==> Ejecting..."
    try {
        $shell = New-Object -ComObject Shell.Application
        $shell.Namespace(17).ParseName("${driveLetter}:").InvokeVerb('Eject')
    } catch {
        Write-Warning "Could not auto-eject the drive ($_). It's safe to remove manually once Windows shows no activity on it."
    }

    Write-Host ""
    Write-Host "Done. Disk $DiskNumber is now a bootable WISP UEFI shell USB stick."
    Write-Host ""
    Write-Host "IMPORTANT: BOOTX64.EFI is unsigned. On real hardware with Secure Boot"
    Write-Host "enabled, the firmware will refuse to run it. Disable Secure Boot in the"
    Write-Host "target machine's firmware setup (or enroll your own signature) first."
    Write-Host "Boot it by choosing this USB stick from the target PC's one-time boot"
    Write-Host "menu (commonly F12/F10/Esc at power-on)."
}

if ($Install) {
    Install-ToDisk -DiskNumber $DiskNumber -Source $Source -Force:$Force.IsPresent
} else {
    Show-DiskList
}
