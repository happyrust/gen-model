# 文件本身：语法、编码、换行。防的是「哪天被某个编辑器改坏了没人发现」。
# 这一组要在 Windows PowerShell 5.1 和 PowerShell 7 下各跑一遍才算数：5.1 的解析器不认 7 才有的写法。

$stackDir = $script:StackDir

Test '所有 .ps1 在当前这个 PowerShell 下语法错误为 0' {
    foreach ($file in @(Get-ChildItem -LiteralPath $stackDir -Recurse -Filter '*.ps1' -File)) {
        $errors = $null
        $tokens = $null
        [void][System.Management.Automation.Language.Parser]::ParseFile($file.FullName, [ref]$tokens, [ref]$errors)
        $first = ''
        if (@($errors).Count -gt 0) { $first = "第 $($errors[0].Extent.StartLineNumber) 行：$($errors[0].Message)" }
        Assert-Equal 0 @($errors).Count "$($file.Name) 的语法错误（$first）"
    }
}

Test '所有 .ps1 都带 BOM（Windows PowerShell 5.1 把没有 BOM 的脚本按本地编码读，中文会乱）' {
    foreach ($file in @(Get-ChildItem -LiteralPath $stackDir -Recurse -Filter '*.ps1' -File)) {
        $bytes = [System.IO.File]::ReadAllBytes($file.FullName)
        $bom = ($bytes.Length -ge 3 -and $bytes[0] -eq 0xEF -and $bytes[1] -eq 0xBB -and $bytes[2] -eq 0xBF)
        Assert-True $bom "$($file.Name) 开头是 UTF-8 的 BOM"
    }
}

Test '批处理文件是纯 ASCII、换行是 CRLF（cmd 按本机代码页读它）' {
    foreach ($file in @(Get-ChildItem -LiteralPath $stackDir -Filter '*.cmd' -File)) {
        $bytes = [System.IO.File]::ReadAllBytes($file.FullName)
        $nonAscii = @($bytes | Where-Object { $_ -gt 127 }).Count
        Assert-Equal 0 $nonAscii "$($file.Name) 里的非 ASCII 字节数"
        $bareLf = 0
        for ($i = 0; $i -lt $bytes.Length; $i++) { if ($bytes[$i] -eq 10 -and ($i -eq 0 -or $bytes[$i - 1] -ne 13)) { $bareLf += 1 } }
        Assert-Equal 0 $bareLf "$($file.Name) 里只有 LF 的换行数"
    }
}

Test '入口报得出版本号，帮助里没有过时的话' {
    Assert-Match $script:StackVersion '^\d+\.\d+\.\d+$' '版本号的样子'
    $text = [System.IO.File]::ReadAllText($script:StackScript, [System.Text.Encoding]::UTF8)
    Assert-NotMatch $text '后端现在这一版（0\.1\.44）' '没有写死「后端 0.1.44 的已知问题」'
    Assert-NotMatch $text '删掉文件夹 \$\(\$Cfg\.Database\.DataDir\)' '没有劝人删数据库目录'
}

Test '包里那份缺省配置、三份样例配置都解析得出来' {
    foreach ($name in 'package\stack.default.toml', 'stack.example.toml', 'stack.sandbox.example.toml', 'stack.dev-live.example.toml') {
        $path = Join-Path $stackDir $name
        Assert-True (Test-Path -LiteralPath $path -PathType Leaf) "$name 在"
        $cfg = Get-StackConfig $path
        Assert-True ($cfg.Backend.Port -gt 0) "$name 解析出了后端端口"
    }
}
