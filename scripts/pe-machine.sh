#!/bin/sh
# pe-machine.sh — answer the ARCHITECTURE a Windows PE binary was built for,
# by reading its COFF header. Prints one token on stdout: `arm64`, `x64`,
# `x86`, `arm32`, or `unknown:<hex>`; exits 2 if the file is not a PE.
#
# WINARM (P3 D5). `docs/sprints/sprint_p03_detail.md` §D5: any Cog/Pharo
# figure recorded on this platform must carry whether the Cog process was
# NATIVE ARM64 or x86-64 running under Windows' x64 translation layer,
# because an emulated Cog loses a large factor to translation alone and
# beating it demonstrates far less than the number suggests. Version strings
# do not answer that question — Pharo's own x86-64 build reports the same
# version whether it is running natively on an Intel box or emulated here —
# so the harness has to ask the binary, not the VM.
#
# Why the PE header and not `IsWow64Process2`: this needs no running process,
# no Win32 call, and no new dependency (MIGRATION.md's standing rule), and it
# answers for a downloaded VM before it has ever been launched. It reads
# exactly two fields, both fixed by the PE/COFF specification (Microsoft,
# "PE Format"):
#
#   * offset 0x3C, 4 bytes LE  -> e_lfanew, the file offset of the PE header
#   * e_lfanew + 0, 4 bytes    -> the signature, which must be "PE\0\0"
#   * e_lfanew + 4, 2 bytes LE -> IMAGE_FILE_HEADER.Machine
#
# Machine constants (winnt.h): 0x8664 IMAGE_FILE_MACHINE_AMD64,
# 0xAA64 IMAGE_FILE_MACHINE_ARM64, 0x014C I386, 0x01C0 ARM, 0x01C4 ARMNT.
#
# NOTE the one thing this cannot tell you: an ARM64EC or ARM64X binary also
# reports 0xAA64 while containing x64 code. Neither Pharo nor OpenSmalltalk
# ships such a build today, and the distinction does not arise for the
# question D5 asks (native-vs-emulated), but a future harness that starts
# seeing ARM64EC should switch to `IsWow64Process2` on the LIVE process.
set -eu

[ $# -eq 1 ] || { echo "usage: pe-machine.sh <file.exe|file.dll>" >&2; exit 2; }
f=$1
[ -r "$f" ] || { echo "pe-machine.sh: cannot read $f" >&2; exit 2; }

# `od -An -tu1 -j<off> -N<n>` gives whitespace-separated decimal bytes; the
# arithmetic below is plain POSIX shell, no awk/python dependency.
bytes_at() { od -An -tu1 -j "$1" -N "$2" "$f" | tr -s ' ' '\n' | grep -v '^$'; }

set -- $(bytes_at 60 4)
lfanew=$(( $1 + ($2 << 8) + ($3 << 16) + ($4 << 24) ))

set -- $(bytes_at "$lfanew" 4)
# "PE\0\0" == 80 69 0 0
if [ "$1" != 80 ] || [ "$2" != 69 ] || [ "$3" != 0 ] || [ "$4" != 0 ]; then
    echo "pe-machine.sh: $f is not a PE image (no PE signature at $lfanew)" >&2
    exit 2
fi

set -- $(bytes_at $(( lfanew + 4 )) 2)
machine=$(( $1 + ($2 << 8) ))

case "$machine" in
    43620) echo arm64 ;;   # 0xAA64
    34404) echo x64   ;;   # 0x8664
    332)   echo x86   ;;   # 0x014C
    448|452) echo arm32 ;; # 0x01C0, 0x01C4
    *)     printf 'unknown:%#x\n' "$machine" ;;
esac
