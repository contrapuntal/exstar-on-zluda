$events = Get-WinEvent -FilterHashtable @{LogName='Application'; Level=1,2,3; StartTime=(Get-Date).AddHours(-3)} -ErrorAction SilentlyContinue
$events | Where-Object {
    $_.Message -like '*EXStar*' -or
    $_.Message -like '*zluda*' -or
    $_.ProviderName -like '*Application Error*' -or
    $_.ProviderName -like '*.NET Runtime*' -or
    $_.Id -eq 1000 -or $_.Id -eq 1026
} | Select-Object -First 20 | ForEach-Object {
    Write-Host "---"
    Write-Host "Time: $($_.TimeCreated)  Id: $($_.Id)  Provider: $($_.ProviderName)"
    $msg = $_.Message
    if ($msg.Length -gt 800) { $msg = $msg.Substring(0, 800) + '...' }
    Write-Host $msg
}
