# Dot-sourced by verify-vault-list-ui.ps1; uses its isolated MSIX, loopback fixture and evidence helpers.
function InvokeFolderUiChecks {
    function NavigateFolderTest([string]$Selector) {
        for ($attempt = 0; $attempt -lt 3; $attempt++) {
            if ((Tree) -notmatch [regex]::Escape($Selector)) { Click 'PART_PaneToggleButton' }
            [void](WaitUi ([regex]::Escape($Selector)))
            try { Click $Selector; return }
            catch {
                # Selecting the same navigation destination is idempotent, unlike submitting a write dialog.
                if ($attempt -eq 2 -or $_.Exception.Message -notmatch 'no longer accessible') { throw }
            }
        }
    }
    function CreateFolderTest([string]$Name) {
        Click 'CreateFolderButton'
        [void](WaitUi '文件夹名称')
        SetValue 'FolderNameInput' $Name
        Click 'PrimaryButton'
        [void](WaitUi '文件夹已创建')
    }
    function WaitFolderLink([string]$Name) {
        $deadline = [DateTime]::UtcNow.AddSeconds(10)
        do {
            try {
                [void](WinApp @('ui', 'get-property', ('打开文件夹“' + $Name + '”'), '-w',
                    [string](Get-Process -Id $previewId).MainWindowHandle, '--type', 'Hyperlink', '--property', 'Name'))
                return
            } catch { }
            Start-Sleep -Milliseconds 200
        } while ([DateTime]::UtcNow -lt $deadline)
        throw "Folder link did not appear: $Name"
    }
    function PopupFolderTest([string]$Name) {
        for ($attempt = 0; $attempt -lt 3; $attempt++) {
            try {
                [void](WinApp @('ui', 'invoke', $Name, '-a', [string]$previewId, '--type', 'ListItem'))
                return
            } catch {
                # Choosing the same folder is idempotent; this never repeats an editor save.
                if ($attempt -eq 2 -or $_.Exception.Message -notmatch 'no longer accessible') { throw }
                [void](WinApp @('ui', 'inspect', '-a', [string]$previewId, '--depth', '20'))
                Start-Sleep -Milliseconds 200
            }
        }
    }
    function EnabledFolderTest([string]$Selector) {
        $hwnd = (Get-Process -Id $previewId).MainWindowHandle
        $properties = WinApp @('ui', 'get-property', $Selector, '-w', [string]$hwnd, '--property', 'IsEnabled')
        if ($properties -notmatch '(?im)^\s*IsEnabled\s*[:=]\s*(True|False)\s*$') { throw "Cannot read IsEnabled: $properties" }
        return $Matches[1] -eq 'True'
    }
    function SetTrayFolderTest([bool]$Enabled) {
        NavigateFolderTest 'SettingsItem'
        [void](WaitUi 'KeepInTrayToggle')
        $action = if ($Enabled) { 'toggle-on' } else { 'toggle-off' }
        [void](WinApp @('ui', 'invoke', 'KeepInTrayToggle', '-w', [string](Get-Process -Id $previewId).MainWindowHandle, '--action', $action))
    }
    function FolderSubtree([string]$Selector) {
        WinApp @('ui', 'inspect', $Selector, '-w', [string](Get-Process -Id $previewId).MainWindowHandle, '--depth', '10')
    }
    function DeleteFolderTest([string]$Name) {
        Click ('删除文件夹“' + $Name + '”') 'Button'
        $dialog = WaitUi '只删除所选文件夹'
        Check ($dialog -match '未选中的嵌套子文件夹及其条目会保留') "delete confirmation explains non-cascading behavior for $Name"
        Click 'PrimaryButton'
        [void](WaitUi '已删除 1 个文件夹')
    }

    NavigateFolderTest 'FolderManagementNavigationItem'
    [void](WaitUi 'FolderCount')
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    while (-not (EnabledFolderTest 'CreateFolderButton')) {
        if ([DateTime]::UtcNow -ge $deadline) { throw 'Folder manager did not finish loading.' }
        Start-Sleep -Milliseconds 200
    }
    if ((Tree) -notmatch 'FolderCount Text "(\d+) 个顶层文件夹，共 (\d+) 个文件夹"') { throw 'Missing folder totals.' }
    $expectedTotals = "$([int]$Matches[1] + 2) 个顶层文件夹，共 $([int]$Matches[2] + 4) 个文件夹"
    CreateFolderTest '工作'
    CreateFolderTest '工作/项目'
    CreateFolderTest '工作/项目/验证'
    CreateFolderTest '失配/孤立'
    $tree = Tree
    Check ($tree.Contains($expectedTotals) -and $tree -match 'ListItem "工作"' -and
        $tree -notmatch 'ListItem "工作/项目"') 'My folders shows real roots rather than every nested folder as a sibling'
    Check ($tree -match 'ListItem "失配/孤立"' -and $tree -notmatch 'ListItem "失配"') 'missing parents stay one full-path root without a fabricated folder'
    Check ($tree -match '0 个条目（含子文件夹），1 个子文件夹') 'root table displays the actual direct child count and explains subtree totals'
    SetValue 'TextBox' '工作/项目'
    [void](WaitUi '2 个匹配文件夹')
    WaitFolderLink '工作/项目'
    WaitFolderLink '工作/项目/验证'
    Tree | Set-Content (Join-Path $run 'folder-search.txt')
    Check $true 'full-name folder search still reaches children and grandchildren'
    SetValue 'TextBox' ''

    NavigateFolderTest 'VaultNavigationItem'
    [void](WaitUi 'VaultList')
    Click 'Synthetic login' 'ListItem'
    [void](WaitUi '修改时间')
    Keys 'ctrl+e'
    [void](WaitUi 'VaultEditorFolder')
    Click 'VaultEditorFolder'
    PopupFolderTest '工作/项目'
    Check ((WaitValue 'VaultEditorFolder' '工作/项目') -eq '工作/项目') 'item editor assigns the exact nested folder full name'
    Click 'PrimaryButton'
    [void](WaitUi '条目已保存')
    NavigateFolderTest 'FolderManagementNavigationItem'
    $tree = WaitUi '1 个条目（含子文件夹），1 个子文件夹'
    Check ($tree -match 'ListItem "工作"') 'parent table total includes the item stored in its child'
    [void](WinApp @('ui', 'screenshot', '-a', [string]$previewId, '-o', (Join-Path $run 'folder-roots-compact.png')))
    Click '打开文件夹“工作”' 'Hyperlink'
    [void](WaitUi 'ChildFolderList')
    $children = FolderSubtree 'ChildFolderList'
    $vault = FolderSubtree 'VaultList'
    Check ($children -match 'ListItem "工作/项目"' -and $children -match '项目' -and $vault -notmatch 'Synthetic login') `
        'opening a parent shows its real child but does not mix child items into the parent list'
    Click '工作/项目' 'ListItem'
    $tree = WaitUi 'ListItem "Synthetic login"'
    Check ((WaitValue 'FolderFilter' '工作/项目') -eq '工作/项目' -and (FolderSubtree 'ChildFolderList') -match '工作/项目/验证') `
        'child navigation selects its exact ID and exposes its grandchild'
    Click 'Synthetic login' 'ListItem'
    $tree = WaitUi '通行密钥'
    Check ($tree -match 'fixture-user') 'nested assignment preserves the login and its passkey detail'
    Click 'DetailBackButton'
    $crumbs = FolderSubtree 'FolderCrumbs'
    Check ($crumbs -match '工作' -and $crumbs -match '项目' -and $crumbs -notmatch 'Id =') 'breadcrumb uses readable real ancestors rather than model dumps'
    [void](WinApp @('ui', 'screenshot', '-a', [string]$previewId, '-o', (Join-Path $run 'folder-child-compact.png')))
    Click '工作' 'Button'
    Check ((WaitValue 'FolderFilter' '工作') -eq '工作' -and (FolderSubtree 'VaultList') -notmatch 'Synthetic login') `
        'breadcrumb back navigation restores the parent exact-ID projection'
    Click '我的文件夹' 'Button'
    [void](WaitUi 'FolderList')
    Check ((Tree).Contains($expectedTotals)) 'root breadcrumb returns to the folder management page'

    # A graceful process restart leaves this synthetic account as locked, sign-in-required cache.
    SetTrayFolderTest $false
    NavigateFolderTest 'VaultNavigationItem'
    [void](WaitUi 'VaultList')
    $app = Get-Process -Id $previewId
    if (-not $app.CloseMainWindow() -or -not $app.WaitForExit(10000)) { throw 'Folder cache test preview did not exit normally.' }
    $launch = WinApp @('run', $layout, '--detach', '--devtools', 'off', '--json') | ConvertFrom-Json
    $script:previewId = [int]$launch.ProcessId
    WaitPreviewWindow
    [void](WaitUi '保险库已锁定')
    Resize 1000 1500
    $before = (Invoke-RestMethod "$baseUrl/control/snapshot" -TimeoutSec 2).syncRequests
    Click 'LockButton'
    [void](WaitUi '仅离线解锁')
    SetValue '主密码' 'asdfasdfasdf'
    Click 'SecondaryButton'
    [void](WaitUi '本机保险库缓存')
    NavigateFolderTest 'FolderManagementNavigationItem'
    $tree = WaitUi '本机文件夹缓存（只读）'
    Check ((EnabledFolderTest 'FolderList') -and -not (EnabledFolderTest 'CreateFolderButton') -and
        -not (EnabledFolderTest '重命名文件夹“工作”') -and -not (EnabledFolderTest '删除文件夹“工作”')) `
        'offline folder cache remains browsable while toolbar and row writes are disabled'
    Click '打开文件夹“工作”' 'Hyperlink'
    [void](WaitUi 'ChildFolderList')
    Click '工作/项目' 'ListItem'
    $tree = WaitUi 'ListItem "Synthetic login"'
    Check ($tree -match '本机保险库缓存' -and (FolderSubtree 'FolderCrumbs') -match '工作' -and
        (Invoke-RestMethod "$baseUrl/control/snapshot" -TimeoutSec 2).syncRequests -eq $before) `
        'offline restart preserves nested names, child navigation and exact membership without a sync request'
    Tree | Set-Content (Join-Path $run 'folder-offline.txt')
    [void](WinApp @('ui', 'screenshot', '-a', [string]$previewId, '-o', (Join-Path $run 'folder-offline.png')))
    Click 'SyncButton'
    [void](WaitUi '重新连接当前账户')
    SetValue '主密码' 'asdfasdfasdf'
    Click 'PrimaryButton'
    [void](WaitUi '保险库已加载')
    SetTrayFolderTest $true
    NavigateFolderTest 'FolderManagementNavigationItem'
    [void](WaitUi 'ListItem "工作"')
    Click '重命名文件夹“工作”' 'Button'
    [void](WaitUi '不会自动修改子文件夹名称')
    Check ((WaitValue 'FolderNameInput' '工作') -eq '工作') 'rename dialog uses the full stored folder name and warns against assumed cascading'
    SetValue 'FolderNameInput' '重命名父'
    Click 'PrimaryButton'
    $tree = WaitUi '文件夹已重命名'
    Check ($tree -match 'ListItem "工作/项目"' -and $tree -match 'ListItem "重命名父"' -and $tree -notmatch 'ListItem "重命名父/项目"') `
        'parent rename preserves child names and makes the old child a joined root'
    DeleteFolderTest '重命名父'
    Check ((Tree) -match 'ListItem "工作/项目"') 'parent deletion preserves the unselected child folder'
    DeleteFolderTest '工作/项目'
    $tree = Tree
    Check ($tree -match 'ListItem "工作/项目/验证"' -and $tree -notmatch 'ListItem "工作/项目"') `
        'child deletion preserves an unselected grandchild as a full-path root'
    NavigateFolderTest 'VaultNavigationItem'
    [void](WaitUi 'VaultList')
    Click 'FolderFilter'
    PopupFolderTest '无文件夹'
    $tree = WaitUi 'ListItem "Synthetic login"'
    Check ((WaitValue 'FolderFilter' '无文件夹') -eq '无文件夹' -and $tree -notmatch 'ChildFolderList') `
        'deleting an assigned folder keeps its item in no-folder without stale breadcrumb or children'
    # Restore the original ciphertext fixture before the existing list/clipboard checks continue.
    Invoke-RestMethod -Method Post "$baseUrl/control/reset" -TimeoutSec 2 | Out-Null
    Click 'SyncButton'
    [void](WaitUi '保险库已加载')
    Click 'FolderFilter'
    PopupFolderTest '所有文件夹'
    [void](WaitUi '2 个条目')
    Check ((Tree) -notmatch 'ChildFolderList') 'sync removes vanished folder navigation metadata and restores the synthetic baseline'
}
