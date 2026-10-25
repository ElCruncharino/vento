# Plays a known tone in the session this runs in, for timing a client's sound
# end to end: half-second bursts of 440 Hz at half scale, one every -Every
# seconds after -Lead seconds of silence, for -Seconds in all, as one WAV
# through System.Media.SoundPlayer. In an RDP session the session's sound is
# what the client hears, so this has to run there, as a task with an
# Interactive principal (usage.md); run from ssh it plays in session 0, where
# it is not heard.
#
# -Out gets two lines, "start <seconds>" just before the sound is handed to
# the player and "end <seconds>" after it has finished, in Unix seconds from
# GetSystemTimePreciseAsFileTime: DateTime.UtcNow moves in steps of the
# timer's 15.6 ms here, which is coarser than what is being measured.
# scripts/testaudio/winbursts.py writes, runs and removes this.
param(
    [int]$Seconds = 60,
    [double]$Every = 2,
    [double]$Lead = 1,
    [string]$Out = "$env:USERPROFILE\vento-tone.txt"
)

Add-Type -Namespace Vento -Name Clock -MemberDefinition @'
[DllImport("kernel32.dll")] static extern void GetSystemTimePreciseAsFileTime(out long t);
public static double Now() { long t; GetSystemTimePreciseAsFileTime(out t); return (t - 116444736000000000L) / 1e7; }
'@

$rate = 48000
$channels = 2
$burst = 0.5
$amplitude = 16384

# One period, burst then silence, built once and repeated: a sample at a time
# in PowerShell is slow enough that five minutes of it would take longer than
# the five minutes.
$periodFrames = [int]($Every * $rate)
$burstFrames = [int]($burst * $rate)
$period = New-Object byte[] ($periodFrames * $channels * 2)
for ($i = 0; $i -lt $burstFrames; $i++) {
    $v = [int16][Math]::Round($amplitude * [Math]::Sin(2 * [Math]::PI * 440 * $i / $rate))
    $lo = [byte]($v -band 0xff)
    $hi = [byte](($v -shr 8) -band 0xff)
    for ($c = 0; $c -lt $channels; $c++) {
        $k = ($i * $channels + $c) * 2
        $period[$k] = $lo
        $period[$k + 1] = $hi
    }
}
$count = [int][Math]::Floor($Seconds / $Every)
$leadBytes = [int]($Lead * $rate) * $channels * 2
$dataBytes = $leadBytes + $count * $period.Length

$ms = New-Object System.IO.MemoryStream
$w = New-Object System.IO.BinaryWriter($ms)
$w.Write([Text.Encoding]::ASCII.GetBytes('RIFF'))
$w.Write([int](36 + $dataBytes))
$w.Write([Text.Encoding]::ASCII.GetBytes('WAVEfmt '))
$w.Write([int]16)
$w.Write([int16]1)
$w.Write([int16]$channels)
$w.Write([int]$rate)
$w.Write([int]($rate * $channels * 2))
$w.Write([int16]($channels * 2))
$w.Write([int16]16)
$w.Write([Text.Encoding]::ASCII.GetBytes('data'))
$w.Write([int]$dataBytes)
$w.Write((New-Object byte[] $leadBytes))
for ($n = 0; $n -lt $count; $n++) {
    $w.Write($period)
}
$w.Flush()
$ms.Position = 0

$player = New-Object System.Media.SoundPlayer($ms)
$player.Load()
"start $([Vento.Clock]::Now().ToString('F6', [Globalization.CultureInfo]::InvariantCulture))" | Set-Content $Out
$player.PlaySync()
"end $([Vento.Clock]::Now().ToString('F6', [Globalization.CultureInfo]::InvariantCulture))" | Add-Content $Out
