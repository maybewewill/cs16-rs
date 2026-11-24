param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$ExtraArgs
)

$ErrorActionPreference = 'Stop'
$argStr = ($ExtraArgs | ForEach-Object { if ($_ -match '\s') { "`"$_`"" } else { $_ } }) -join ' '
& 'C:\Program Files\Git\bin\bash.exe' -lc "nice -n 19 env CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CS16_GAMEDIR=`"D:\SteamLibrary\steamapps\common\Half-Life`" cargo xtask visual-diff $argStr"
exit $LASTEXITCODE
