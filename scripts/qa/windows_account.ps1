# Run only on an ephemeral Windows CI runner with local administrator rights.
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true') { throw 'Requires an ephemeral GitHub Actions runner' }
$artifacts = cargo test --locked -p removent-proto -p removent-core -p removent-input -p removent-client -p removent-host -p removent-media-codec --lib --no-run --message-format=json
if ($LASTEXITCODE -ne 0) { throw 'Account test build failed' }
$binary = $artifacts | ForEach-Object { $_ | ConvertFrom-Json } |
    Where-Object { $_.reason -eq 'compiler-artifact' -and $_.profile.test -and $_.executable -and $_.target.name -eq 'removent_host' } |
    Select-Object -Last 1 -ExpandProperty executable
if (-not $binary) { throw 'Account test executable missing' }
$username = 'rmv' + [Guid]::NewGuid().ToString('N').Substring(0, 12)
$password = [Guid]::NewGuid().ToString('N') + 'aA1!'
$secure = ConvertTo-SecureString $password -AsPlainText -Force
$created = $false
$child = $null
$accountTemp = Join-Path $env:PUBLIC "removent-account-$username"
$tempCreated = $false
$previousTemp = $env:TEMP
$previousTmp = $env:TMP
$evidence = Join-Path (Get-Location) 'target/qa-evidence'
New-Item -ItemType Directory -Force $evidence | Out-Null
try {
    Start-Service seclogon
    New-LocalUser -Name $username -Password $secure -Description 'Ephemeral Removent authentication test' | Out-Null
    $created = $true
    $users = Get-LocalGroup -SID 'S-1-5-32-545'
    Add-LocalGroupMember -Group $users -Member $username
    # Start-Process inherits the runner's private TEMP, which the test user cannot
    # access. Give the QUIC identity fixture its own directory and explicit ACL.
    New-Item -ItemType Directory $accountTemp | Out-Null
    $tempCreated = $true
    $runnerSid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
    $testSid = (Get-LocalUser -Name $username).SID.Value
    icacls $accountTemp /inheritance:r /grant "*${runnerSid}:(OI)(CI)F" "*${testSid}:(OI)(CI)F" '*S-1-5-18:(OI)(CI)F' | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'Cannot protect account test directory' }
    $env:TEMP = $accountTemp
    $env:TMP = $accountTemp
    $env:REMOVENT_AUTH_TEST_USERNAME = $username
    $env:REMOVENT_AUTH_TEST_PASSWORD = $password
    $env:REMOVENT_AUTH_EXPECT_REJECT = '1'
    & $binary native_desktop_account_authentication --ignored --nocapture
    if ($LASTEXITCODE -ne 0) { throw 'A different account was accepted as the desktop owner' }
    Remove-Item Env:REMOVENT_AUTH_EXPECT_REJECT
    $credential = [PSCredential]::new("$env:COMPUTERNAME\$username", $secure)
    $child = Start-Process -FilePath $binary -ArgumentList 'native_desktop_account_authentication --ignored --nocapture' -Credential $credential -PassThru -WorkingDirectory (Get-Location).Path -RedirectStandardOutput "$evidence/account.log" -RedirectStandardError "$evidence/account-errors.log"
    if (-not $child.WaitForExit(30000)) { throw 'Native account test timed out' }
    Get-Content "$evidence/account.log"
    Get-Content "$evidence/account-errors.log"
    if ($child.ExitCode -ne 0) { throw 'Native account authentication failed' }
    Write-Output 'PASS: Windows account/QUIC password, invalid password and desktop-owner SID checks'
} finally {
    $env:TEMP = $previousTemp
    $env:TMP = $previousTmp
    if ($child -and -not $child.HasExited) { Stop-Process -Id $child.Id -Force }
    foreach ($name in @('REMOVENT_AUTH_TEST_USERNAME', 'REMOVENT_AUTH_TEST_PASSWORD', 'REMOVENT_AUTH_EXPECT_REJECT')) {
        Remove-Item "Env:$name" -ErrorAction SilentlyContinue
    }
    if ($created) { Remove-LocalUser -Name $username }
    if ($tempCreated) { Remove-Item -Recurse -Force $accountTemp }
}
