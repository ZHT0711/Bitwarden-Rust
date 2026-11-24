# Dot-sourced by verify-vault-list-ui.ps1 after login to the linked-field fixture.
function InvokeLinkedFieldUiChecks {
    function LinkedUi([string[]]$Arguments) {
        $hwnd = [string](Get-Process -Id $previewId).MainWindowHandle
        WinApp (@('ui') + $Arguments + @('-w', $hwnd))
    }

    function LinkedValue([string]$Label) {
        (LinkedUi @('get-value', $Label, '--type', 'Edit')).Trim()
    }

    function OpenLinkedItem([string]$Name) {
        Click $Name 'ListItem'
        WaitUi '修改时间'
    }

    function ReturnToLinkedList {
        Click 'DetailBackButton'
        [void](WaitUi 'VaultList')
    }

    Resize 1000 1500
    Check ((WaitUi '6 个条目') -match 'VaultList') 'linked-field MSIX loads all six synthetic entries'
    $readSyncs = (Invoke-RestMethod "$baseUrl/control/snapshot" -TimeoutSec 2).syncRequests
    foreach ($query in @('linked-101', 'synthetic-password-DO-NOT-USE')) {
        SetValue 'TextBox' $query
        $filtered = WaitUi '没有匹配的条目'
        Check ($filtered -match '0 个条目') "search excludes synthetic linked data: $query"
    }
    SetValue 'TextBox' ''
    [void](WaitUi '6 个条目')

    $login = OpenLinkedItem 'Synthetic login'
    Check ((LinkedValue 'linked-100') -eq 'fixture-user' -and $login -notmatch 'stale-linked-storage-value') `
        'login detail resolves the live username and omits stale literal data'
    $mask = LinkedUi @('get-property', 'linked-101', '--type', 'Edit', '--property', 'IsPassword')
    Check ($mask -match '(?im)^\s*IsPassword\s*[:=]\s*True\s*$' -and $login -match '显示linked-101') `
        'linked login password starts masked behind its own reveal toggle'
    [void](LinkedUi @('invoke', '显示linked-101', '--type', 'Button', '--action', 'toggle-on'))
    Check ((WaitUi 'Button "隐藏linked-101"') -match 'Button "隐藏linked-101"') 'linked password can be revealed explicitly'
    [void](LinkedUi @('invoke', '隐藏linked-101', '--type', 'Button', '--action', 'toggle-off'))
    Check ((WaitUi 'Button "显示linked-101"') -match 'Button "显示linked-101"') 'linked password can be masked again'
    Click '复制linked-100' 'Button'
    Check ((Get-Clipboard -Raw) -eq 'fixture-user') 'copy uses the resolved linked username'
    [void](LinkedUi @('scroll', 'DetailScroll', '--to', 'bottom'))
    [void](WinApp @('ui', 'screenshot', '-a', [string]$previewId, '-o', (Join-Path $run 'linked-login-masked.png')))
    ReturnToLinkedList

    $card = OpenLinkedItem 'Synthetic linked card'
    Check ((LinkedValue 'linked-300') -eq 'Public Cardholder' -and $card -match '显示linked-303' -and $card -match '显示linked-305') `
        'card detail resolves the cardholder and masks linked security code and number'
    ReturnToLinkedList

    $identityDetail = OpenLinkedItem 'Synthetic linked identity'
    Check ((LinkedValue 'linked-418') -eq 'Dr Sample Public Person' -and $identityDetail -match '显示linked-412' -and
        $identityDetail -match '显示linked-414' -and $identityDetail -match '显示linked-415') `
        'identity detail resolves the composite name and masks linked document identifiers'
    ReturnToLinkedList

    $hidden = OpenLinkedItem 'Synthetic hidden links'
    Check ($hidden -match '组织权限禁止查看密码和隐藏字段' -and (LinkedValue 'linked-100') -eq 'fixture-user' -and
        $hidden -notmatch 'linked-101|synthetic-password-DO-NOT-USE') `
        'hidden-password permission removes the linked secret and keeps the linked username'
    ReturnToLinkedList

    $partial = OpenLinkedItem 'Synthetic partial links'
    Check ($partial -match '此条目受组织访问策略限制' -and $partial -notmatch 'linked-10[01]|synthetic-password-DO-NOT-USE') `
        'restricted organization detail exposes no linked fields'
    ReturnToLinkedList

    Click 'Synthetic protected item' 'ListItem'
    $prompt = WaitUi '验证主密码'
    Check ($prompt -notmatch 'linked-10[01]') 'master-password prompt appears before the linked detail'
    SetValue '主密码' 'asdfasdfasdf'
    Click 'PrimaryButton'
    Check ((WaitUi '修改时间') -match '显示linked-101') 'successful master-password reprompt opens a masked linked secret'
    Check ((Invoke-RestMethod "$baseUrl/control/snapshot" -TimeoutSec 2).syncRequests -eq $readSyncs) `
        'search and linked detail reads do not make an extra vault sync request'
    ReturnToLinkedList

    [void](OpenLinkedItem 'Synthetic login')
    Click 'EditItemButton'
    [void](WaitUi 'VaultEditor_username')
    # Save only once. The saved detail is projected again from the native association.
    SetValue 'VaultEditor_username' 'updated-linked-user'
    Click 'PrimaryButton'
    [void](WaitUi '条目已保存')
    Check ((WaitUi '修改时间') -match '显示linked-101' -and (LinkedValue 'linked-100') -eq 'updated-linked-user') `
        'saving a source edit refreshes the linked value in the real detail page'

    Click 'LockButton'
    $locked = WaitUi '保险库已锁定'
    Check ($locked -notmatch 'linked-10[01]|updated-linked-user|ListItem.*"Synthetic') 'locking removes linked controls and vault results'
}
