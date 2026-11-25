using Bytewarden.WinUI.Interop;
using Bytewarden.WinUI.Controls;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace Bytewarden.WinUI.Pages;

public sealed partial class HomePage
{
    private bool _busy;
    private bool _editorOpen;
    private bool _requiresSync;
    private bool _selectingItems;
    private bool _abortAction;
    private VaultEditorDialog? _activeEditor;
    private ContentDialog? _actionDialog;

    private void UpdateMutationCommands()
    {
        if (NewItemButton is null) return;
        var available = _loaded && _overview is not null && !_busy && !_syncing && !_editorOpen && !_requiresSync;
        var inTrash = _category == "trash";
        SelectItemsButton.IsEnabled = available;
        SelectItemsButton.Label = _selectingItems ? "退出批量选择" : "批量选择";
        var selectedItems = _selectingItems ? VaultList.SelectedItems.OfType<VaultItem>().ToArray() : [];
        MoveItemsButton.IsEnabled = available && !inTrash && VaultList.SelectedItems.Count > 0;
        var item = _selectingItems ? null : VaultList.SelectedItem as VaultItem;
        NewItemButton.IsEnabled = available && !_selectingItems;
        EditItemButton.IsEnabled = available && !_selectingItems && item is { Deleted: false, Restricted: false };
        CloneItemButton.IsEnabled = available && !_selectingItems && item is { Deleted: false, Restricted: false };
        FavoriteItemButton.IsEnabled = ArchiveItemButton.IsEnabled = available && !_selectingItems && item is { Deleted: false };
        TrashItemButton.IsEnabled = _selectingItems
            ? available && !inTrash && selectedItems.Length > 0 && selectedItems.All(entry => !entry.Deleted && !entry.Restricted)
            : available && item is { Deleted: false, Restricted: false };
        RestoreItemButton.IsEnabled = _selectingItems
            ? available && inTrash && selectedItems.Length > 0 && selectedItems.All(entry => entry.Deleted && !entry.Restricted)
            : available && item is { Deleted: true, Restricted: false };
        DeleteItemButton.IsEnabled = _selectingItems
            ? available && inTrash && selectedItems.Length > 0 && selectedItems.All(entry => entry.Deleted && !entry.Restricted)
            : available && item is { Deleted: true, Restricted: false };
        var trashCount = inTrash && _overview is not null ? _overview.Items.Count(entry => entry.Deleted && !entry.Restricted) : 0;
        EmptyTrashButton.Visibility = inTrash ? Visibility.Visible : Visibility.Collapsed;
        EmptyTrashButton.IsEnabled = available && !_selectingItems && trashCount > 0;
        var deleted = inTrash || item?.Deleted == true;
        MoveItemsButton.Visibility = inTrash ? Visibility.Collapsed : Visibility.Visible;
        CloneItemButton.Visibility = inTrash ? Visibility.Collapsed : Visibility.Visible;
        FavoriteItemButton.Visibility = ArchiveItemButton.Visibility = TrashItemButton.Visibility = deleted ? Visibility.Collapsed : Visibility.Visible;
        RestoreItemButton.Visibility = DeleteItemButton.Visibility = deleted ? Visibility.Visible : Visibility.Collapsed;
        FavoriteItemButton.Label = item?.Favorite == true ? "取消收藏" : "收藏";
        ArchiveItemButton.Label = item?.Archived == true ? "取消归档" : "归档";
        var showQuickCopy = available && !_selectingItems && item is { Restricted: false, ItemType: 1 } &&
            DesktopPreferences.Read(DesktopPreferences.ShowQuickCopyActions, true);
        QuickCopyBar.Visibility = showQuickCopy ? Visibility.Visible : Visibility.Collapsed;
        QuickCopyUserButton.IsEnabled = QuickCopyPasswordButton.IsEnabled = QuickCopyTotpButton.IsEnabled = showQuickCopy;
    }

    private void AbortMutationFlow()
    {
        _abortAction = true;
        _activeEditor?.Abort();
        _actionDialog?.Hide();
    }

    private async void NewItem_Click(object sender, RoutedEventArgs e) => await OpenEditorAsync(null);
    private async void EditItem_Click(object sender, RoutedEventArgs e)
    { if (VaultList.SelectedItem is VaultItem item) await OpenEditorAsync(item); }
    private async void CloneItem_Click(object sender, RoutedEventArgs e)
    { if (VaultList.SelectedItem is VaultItem item) await OpenEditorAsync(item, clone: true); }

    private void SelectItems_Click(object sender, RoutedEventArgs e)
    {
        if (!SelectItemsButton.IsEnabled) return;
        CancelOperation();
        _version++;
        SetItemSelectionMode(!_selectingItems);
        DetailPanel.Children.Clear();
        DetailPanel.Children.Add(new TextBlock
        {
            Text = _selectingItems ? (_category == "trash" ? "选择要还原或永久删除的条目" : "选择要移动或移至回收站的条目") : "选择一个条目查看详情",
        });
        UpdateMutationCommands();
    }

    private void SetItemSelectionMode(bool multiple)
    {
        if (_selectingItems) VaultList.SelectedItems.Clear();
        else VaultList.SelectedItem = null;
        _selectingItems = multiple;
        VaultList.SelectionMode = multiple ? ListViewSelectionMode.Multiple : ListViewSelectionMode.Single;
    }

    private async void MoveItems_Click(object sender, RoutedEventArgs e)
    {
        if (!MoveItemsButton.IsEnabled || _overview is null) return;
        var items = VaultList.SelectedItems.OfType<VaultItem>().ToArray();
        if (items.Length == 0) return;
        var previousSelection = _selectingItems ? null : items[0];
        if (items.Length > 500)
        {
            SetNotice("所选条目过多", "每次最多可移动 500 个条目。", InfoBarSeverity.Warning);
            return;
        }

        var folders = new ComboBox
        {
            Header = "目标文件夹", MinWidth = 240, MaxWidth = 480,
            HorizontalAlignment = HorizontalAlignment.Stretch,
            ItemTemplate = FolderFilter.ItemTemplate,
            ItemsSource = FolderChoice.ForEditor(_overview.Folders),
            SelectedIndex = 0,
        };
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetAutomationId(folders, "VaultMoveFolder");
        var dialog = _actionDialog = new ContentDialog
        {
            XamlRoot = XamlRoot,
            Title = $"移动 {items.Length} 个条目",
            Content = folders,
            PrimaryButtonText = "移动",
            CloseButtonText = "取消",
            DefaultButton = ContentDialogButton.Primary,
        };

        CancelOperation();
        var version = ++_version;
        using var operation = _operation = new CancellationTokenSource();
        var token = operation.Token;
        _editorOpen = true;
        SetBusy(true);
        DetailPanel.Children.Clear();
        try
        {
            if (await dialog.ShowAsync() != ContentDialogResult.Primary || !IsCurrent(version, token)) return;
            var target = (folders.SelectedItem as FolderChoice)?.Id is { Length: > 0 } folder ? folder : null;
            folders.ItemsSource = null;
            _actionDialog = null;
            var ids = items.Select(item => item.Id).ToArray();
            var result = await Task.Run(() => RustCore.MoveVaultItems(ids, target, token), token);
            if (!_loaded || version != _version) return;
            previousSelection = null;
            SetItemSelectionMode(false);
            ApplyMutationResult(result, $"已移动 {items.Length} 个条目");
        }
        catch (OperationCanceledException)
        {
            if (_loaded && version == _version)
            {
                _requiresSync = true;
                SetNotice("请求已取消", "请先同步并核对移动结果。", InfoBarSeverity.Warning);
            }
        }
        catch (VaultException error) { if (_loaded && version == _version) MutationError(error); }
        catch (Exception error) when (IsCoreUnavailable(error))
        {
            if (IsCurrent(version, token)) SetNotice("核心不可用", "请修复应用安装后重试。", InfoBarSeverity.Error);
        }
        finally
        {
            folders.ItemsSource = null;
            if (ReferenceEquals(_actionDialog, dialog)) _actionDialog = null;
            if (ReferenceEquals(_operation, operation)) _operation = null;
            if (version == _version)
            {
                _editorOpen = false;
                SetBusy(false);
                if (previousSelection is not null && !_requiresSync) RestoreSelection(previousSelection);
            }
        }
    }

    private async Task OpenEditorAsync(VaultItem? item, bool clone = false)
    {
        if (_editorOpen || _syncing || _busy || _requiresSync || _overview is null) return;
        var previousSelection = VaultList.SelectedItem as VaultItem;
        CancelOperation();
        var version = ++_version;
        var cancellation = _operation = new CancellationTokenSource();
        var token = cancellation.Token;
        _editorOpen = true; _abortAction = false; SetBusy(true);
        DetailPanel.Children.Clear();
        var password = string.Empty;
        VaultMutationResult? result = null;
        try
        {
            VaultEditorData data;
            if (item is null)
            {
                var type = _category switch { "logins" => 1, "notes" => 2, "cards" => 3, "identities" => 4,
                    "ssh" => 5, "bankaccounts" => 6, "licenses" => 7, "passports" => 8, _ => 1 };
                var folder = (FolderFilter.SelectedItem as FolderChoice)?.Id;
                data = new VaultEditorData(null, string.Empty, type, folder is "*" or "" ? null : folder,
                    false, false, null, null, [], [], [], true, false, false, false, false);
            }
            else
            {
                if (item.Reprompt)
                {
                    var entered = await PromptMasterPasswordAsync();
                    if (entered is null) throw new OperationCanceledException();
                    password = entered;
                }
                data = await Task.Run(() => RustCore.GetVaultEditor(item.Id, password, token), token);
                token.ThrowIfCancellationRequested();
                if (!_loaded || version != _version) return;
                if (!data.CanEdit || data.IsDeleted) throw new VaultException(data.EditBlockedReason ?? "permissionDenied");
                if (clone)
                {
                    data = data with
                    {
                        Id = null,
                        RevisionDate = null,
                        Name = string.IsNullOrWhiteSpace(data.Name) ? "副本" : $"{data.Name} - 副本",
                        Passkeys = null,
                        RemoveSavedPasskey = false,
                        CanEdit = true,
                        IsDeleted = false,
                        EditBlockedReason = null,
                    };
                }
            }
            _activeEditor = VaultEditorDialog.Create(XamlRoot, data, _overview.Folders, FolderFilter.ItemTemplate, async (draft, requestToken) =>
            {
                try
                {
                    return await Task.Run(() => (item is null || clone) ? RustCore.CreateVaultItem(draft, requestToken)
                        : RustCore.UpdateVaultItem(draft, password, requestToken), requestToken);
                }
                catch (VaultException error)
                {
                    if (error.Code is "conflict" or "writeUncertain" or "notSynced") _requiresSync = true;
                    throw;
                }
                catch (OperationCanceledException) { _requiresSync = true; throw; }
            }, () => { if (_loaded && version == _version) _requiresSync = true; });
            result = await _activeEditor.ShowAsync(token);
        }
        catch (OperationCanceledException) { }
        catch (VaultException error) { if (IsCurrent(version, token)) MutationError(error); }
        catch (Exception error) when (IsCoreUnavailable(error)) { if (IsCurrent(version, token)) SetNotice("核心不可用", "请修复应用安装后重试。", InfoBarSeverity.Error); }
        finally
        {
            password = string.Empty; _activeEditor = null;
            if (ReferenceEquals(_operation, cancellation)) _operation = null;
            cancellation.Dispose();
            if (version == _version) { _editorOpen = false; SetBusy(false); }
        }
        if (!_loaded || version != _version) return;
        if (result is not null) ApplyMutationResult(result, clone ? "已克隆条目" : "条目已保存");
        else if (_requiresSync) SetNotice("需要同步核对", "条目可能已在服务器更新。请先同步，再进行其他写入。", InfoBarSeverity.Warning);
        else RestoreSelection(item ?? previousSelection);
    }

    private async void ItemAction_Click(object sender, RoutedEventArgs e)
    {
        if (sender is not AppBarButton { Tag: string requested }) return;
        if (_selectingItems)
        {
            if (requested is "trash" or "restore" or "delete")
            {
                var items = VaultList.SelectedItems.OfType<VaultItem>().ToArray();
                if (items.Length > 0) await ExecuteBulkActionAsync(items, requested);
            }
            return;
        }
        if (VaultList.SelectedItem is not VaultItem item) return;
        await ExecuteItemActionAsync(item, requested);
    }

    private async void EmptyTrash_Click(object sender, RoutedEventArgs e)
    {
        if (!EmptyTrashButton.IsEnabled || _overview is null) return;
        var trashItems = _overview.Items.Where(entry => entry.Deleted && !entry.Restricted).ToArray();
        if (trashItems.Length == 0) return;
        await ExecuteBulkActionAsync(trashItems, "delete", titleOverride: $"清空回收站中的 {trashItems.Length} 个条目？");
    }

    private async Task ExecuteBulkActionAsync(VaultItem[] items, string action, string? titleOverride = null)
    {
        if (_editorOpen || _syncing || _busy || _requiresSync || _overview is null || items.Length == 0) return;
        if (items.Length > 500)
        {
            SetNotice("所选条目过多", "每次批量操作最多支持 500 个条目。", InfoBarSeverity.Warning);
            return;
        }
        CancelOperation();
        var version = ++_version;
        var cancellation = _operation = new CancellationTokenSource();
        var token = cancellation.Token;
        _editorOpen = true; _abortAction = false; SetBusy(true);
        DetailPanel.Children.Clear();
        var password = string.Empty;
        VaultMutationResult? result = null;
        var writeDispatched = false;
        var label = action switch { "trash" => "移至回收站", "restore" => "还原", _ => "永久删除" };
        try
        {
            if (items.Any(entry => entry.Reprompt))
            {
                var entered = await PromptMasterPasswordAsync();
                if (entered is null) throw new OperationCanceledException();
                password = entered;
            }
            var notice = new InfoBar { IsClosable = false, IsOpen = false, Severity = InfoBarSeverity.Error };
            var panel = new StackPanel { Spacing = 12, MinWidth = 300, MaxWidth = 480 };
            panel.Children.Add(new TextBlock
            {
                Text = titleOverride ?? $"确定要{label}所选的 {items.Length} 个条目？",
                TextWrapping = TextWrapping.Wrap,
            });
            if (action == "delete")
                panel.Children.Add(new TextBlock { Text = "此操作不可撤销。所有被永久删除的条目都无法恢复。", TextWrapping = TextWrapping.Wrap });
            panel.Children.Add(notice);
            var dialog = _actionDialog = new ContentDialog
            {
                XamlRoot = XamlRoot,
                Title = $"批量{label}",
                Content = panel,
                PrimaryButtonText = label,
                CloseButtonText = "取消",
                DefaultButton = ContentDialogButton.Close,
            };
            var busy = false;
            var mustSync = false;
            CancellationTokenSource? request = null;
            using var abort = token.Register(() => dialog.DispatcherQueue.TryEnqueue(() =>
            {
                if (busy) { _requiresSync = true; request?.Cancel(); }
                else { _abortAction = true; dialog.Hide(); }
            }));
            dialog.CloseButtonClick += (_, args) => { if (busy) { args.Cancel = true; _requiresSync = true; request?.Cancel(); } };
            dialog.Closing += (_, args) => { if (busy && !_abortAction) { args.Cancel = true; _requiresSync = true; request?.Cancel(); } };
            dialog.PrimaryButtonClick += async (_, args) =>
            {
                args.Cancel = true;
                if (busy || mustSync) return;
                var deferral = args.GetDeferral();
                request?.Dispose();
                request = CancellationTokenSource.CreateLinkedTokenSource(token);
                busy = true; dialog.IsPrimaryButtonEnabled = false; dialog.CloseButtonText = "取消请求"; notice.IsOpen = false;
                writeDispatched = true;
                try
                {
                    var ids = items.Select(entry => entry.Id).ToArray();
                    result = await Task.Run(() => RustCore.BulkVaultAction(ids, action, password, request.Token), request.Token);
                    busy = false; dialog.Hide();
                }
                catch (OperationCanceledException) { mustSync = _requiresSync = true; notice.Message = "请求已取消，请先同步核对操作结果。不要重复提交。"; notice.IsOpen = true; }
                catch (VaultException error)
                {
                    mustSync = error.Code is "conflict" or "writeUncertain" or "notSynced";
                    _requiresSync |= mustSync; notice.Message = error.UserMessage + (mustSync ? " 请取消并同步核对，不要重复提交。" : string.Empty); notice.IsOpen = true;
                }
                catch (Exception error) when (IsCoreUnavailable(error)) { notice.Message = "Rust 核心未能加载，请修复应用安装。"; notice.IsOpen = true; }
                finally { busy = false; dialog.IsPrimaryButtonEnabled = !mustSync; dialog.CloseButtonText = "取消"; deferral.Complete(); }
            };
            try { await dialog.ShowAsync(); }
            finally { request?.Cancel(); request?.Dispose(); _actionDialog = null; }
        }
        catch (OperationCanceledException)
        {
            if (writeDispatched && _loaded && version == _version) { _requiresSync = true; SetNotice("请求已取消", "请先同步核对服务器状态，再进行其他写入。", InfoBarSeverity.Warning); }
        }
        catch (VaultException error) { if (_loaded && version == _version) MutationError(error); }
        catch (Exception error) when (IsCoreUnavailable(error)) { if (IsCurrent(version, token)) SetNotice("核心不可用", "请修复应用安装后重试。", InfoBarSeverity.Error); }
        finally
        {
            password = string.Empty; _actionDialog = null;
            if (ReferenceEquals(_operation, cancellation)) _operation = null;
            cancellation.Dispose();
            if (version == _version) { _editorOpen = false; SetBusy(false); }
        }
        if (!_loaded || version != _version) return;
        if (result is not null)
        {
            SetItemSelectionMode(false);
            ApplyMutationResult(result, $"已{label} {items.Length} 个条目");
        }
        else if (_requiresSync) SetNotice("需要同步核对", "条目可能已在服务器更新。请先同步，再进行其他写入。", InfoBarSeverity.Warning);
    }

    private async Task ExecuteItemActionAsync(VaultItem item, string requested)
    {
        if (_editorOpen || _syncing || _busy || _requiresSync || _overview is null) return;
        CancelOperation();
        var version = ++_version;
        var cancellation = _operation = new CancellationTokenSource();
        var token = cancellation.Token;
        _editorOpen = true; _abortAction = false; SetBusy(true);
        DetailPanel.Children.Clear();
        var password = string.Empty;
        VaultMutationResult? result = null;
        var title = "条目已更新";
        var writeDispatched = false;
        try
        {
            // Favorites are user metadata: use list revision metadata without opening a secret editor.
            if (requested == "favorite")
            {
                if (string.IsNullOrEmpty(item.RevisionDate)) throw new VaultException("notSynced");
                var action = item.Favorite ? "unfavorite" : "favorite";
                title = item.Favorite ? "已取消收藏" : "已收藏";
                writeDispatched = true;
                result = await Task.Run(() => RustCore.VaultAction(item.Id, action, item.RevisionDate, string.Empty, token), token);
            }
            else
            {
                if (item.Reprompt)
                {
                    var entered = await PromptMasterPasswordAsync();
                    if (entered is null) throw new OperationCanceledException();
                    password = entered;
                }
                var data = await Task.Run(() => RustCore.GetVaultEditor(item.Id, password, token), token);
                token.ThrowIfCancellationRequested();
                if (!_loaded || version != _version) return;
                var action = requested == "archive" ? data.IsArchived ? "unarchive" : "archive" : requested;
                if (action == "archive" && !data.CanArchive)
                {
                    SetNotice("无法归档", "此账户或组织目前没有可用的归档权限。", InfoBarSeverity.Warning);
                }
                else
                {
                    if ((action is "trash" or "delete") && !data.CanDelete || action == "restore" && !data.CanRestore)
                        throw new VaultException("permissionDenied");
                    if (string.IsNullOrEmpty(data.RevisionDate)) throw new VaultException("notSynced");
                    title = action switch { "archive" => "条目已归档", "unarchive" => "已取消归档", "trash" => "条目已移至回收站",
                        "restore" => "条目已还原", "delete" => "条目已永久删除", _ => title };
                    writeDispatched = true;
                    result = await ConfirmActionAsync(item, action, data.RevisionDate, password, token);
                }
            }
        }
        catch (OperationCanceledException)
        {
            if (writeDispatched && _loaded && version == _version) { _requiresSync = true; SetNotice("请求已取消", "请先同步核对服务器状态，再进行其他写入。", InfoBarSeverity.Warning); }
        }
        catch (VaultException error) { if (_loaded && version == _version) MutationError(error); }
        catch (Exception error) when (IsCoreUnavailable(error)) { if (IsCurrent(version, token)) SetNotice("核心不可用", "请修复应用安装后重试。", InfoBarSeverity.Error); }
        finally
        {
            password = string.Empty; _actionDialog = null;
            if (ReferenceEquals(_operation, cancellation)) _operation = null;
            cancellation.Dispose();
            if (version == _version) { _editorOpen = false; SetBusy(false); }
        }
        if (!_loaded || version != _version) return;
        if (result is not null) ApplyMutationResult(result, title);
        else if (_requiresSync) SetNotice("需要同步核对", "条目可能已在服务器更新。请先同步，再进行其他写入。", InfoBarSeverity.Warning);
        else RestoreSelection(item);
    }

    private async Task<string?> PromptMasterPasswordAsync()
    {
        var password = new PasswordBox { Header = "主密码" };
        _actionDialog = new ContentDialog { XamlRoot = XamlRoot, Title = "验证主密码", Content = PasswordVisibility.WithToggle(password),
            PrimaryButtonText = "继续", CloseButtonText = "取消", DefaultButton = ContentDialogButton.Primary };
        try { return await _actionDialog.ShowAsync() == ContentDialogResult.Primary ? password.Password : null; }
        finally { password.Password = string.Empty; _actionDialog = null; }
    }

    private async Task<VaultMutationResult?> ConfirmActionAsync(VaultItem item, string action, string revision, string password, CancellationToken token)
    {
        var permanent = action == "delete";
        var label = action switch { "trash" => "移至回收站", "restore" => "还原", "archive" => "归档", "unarchive" => "取消归档", _ => "永久删除" };
        var notice = new InfoBar { IsClosable = false, IsOpen = false, Severity = InfoBarSeverity.Error };
        var panel = new StackPanel { Spacing = 12, MinWidth = 300, MaxWidth = 480 };
        panel.Children.Add(new TextBlock { Text = $"{label}“{item.DisplayName}”？", TextWrapping = TextWrapping.Wrap });
        if (permanent) panel.Children.Add(new TextBlock { Text = "此操作不可撤销。请确认你确实要永久删除此条目。", TextWrapping = TextWrapping.Wrap });
        panel.Children.Add(notice);
        var dialog = _actionDialog = new ContentDialog { XamlRoot = XamlRoot, Title = label, Content = panel,
            PrimaryButtonText = label, CloseButtonText = "取消", DefaultButton = ContentDialogButton.Close };
        var busy = false;
        var mustSync = false;
        VaultMutationResult? result = null;
        CancellationTokenSource? request = null;
        using var abort = token.Register(() => dialog.DispatcherQueue.TryEnqueue(() =>
        {
            if (busy) { _requiresSync = true; request?.Cancel(); }
            else { _abortAction = true; dialog.Hide(); }
        }));
        dialog.CloseButtonClick += (_, args) => { if (busy) { args.Cancel = true; _requiresSync = true; request?.Cancel(); } };
        dialog.Closing += (_, args) => { if (busy && !_abortAction) { args.Cancel = true; _requiresSync = true; request?.Cancel(); } };
        dialog.PrimaryButtonClick += async (_, args) =>
        {
            args.Cancel = true;
            if (busy || mustSync) return;
            var deferral = args.GetDeferral();
            request?.Dispose();
            request = CancellationTokenSource.CreateLinkedTokenSource(token);
            busy = true; dialog.IsPrimaryButtonEnabled = false; dialog.CloseButtonText = "取消请求"; notice.IsOpen = false;
            try
            {
                result = await Task.Run(() => RustCore.VaultAction(item.Id, action, revision, password, request.Token), request.Token);
                busy = false; dialog.Hide();
            }
            catch (OperationCanceledException) { mustSync = _requiresSync = true; notice.Message = "请求已取消，请先同步核对操作结果。不要重复提交。"; notice.IsOpen = true; }
            catch (VaultException error)
            {
                mustSync = error.Code is "conflict" or "writeUncertain" or "notSynced";
                _requiresSync |= mustSync; notice.Message = error.UserMessage + (mustSync ? " 请取消并同步核对，不要重复提交。" : string.Empty); notice.IsOpen = true;
            }
            catch (Exception error) when (IsCoreUnavailable(error)) { notice.Message = "Rust 核心未能加载，请修复应用安装。"; notice.IsOpen = true; }
            finally { busy = false; dialog.IsPrimaryButtonEnabled = !mustSync; dialog.CloseButtonText = "取消"; deferral.Complete(); }
        };
        try { await dialog.ShowAsync(); return result; }
        finally { request?.Cancel(); request?.Dispose(); _actionDialog = null; }
    }

    private void MutationError(VaultException error)
    {
        _requiresSync |= error.Code is "conflict" or "writeUncertain" or "notSynced";
        SetNotice("无法更新条目", error.UserMessage + (_requiresSync ? " 请先同步并核对服务器状态。" : string.Empty), InfoBarSeverity.Error);
    }

    private void ApplyMutationResult(VaultMutationResult result, string title)
    {
        _overview = result.Overview; _requiresSync = false;
        _syncing = true;
        try { PopulateFolders(); ApplyFilters(); }
        finally { _syncing = false; }
        SetNotice(title, "服务器已确认操作，列表已更新。", InfoBarSeverity.Success);
        SetBusy(false);
        var selected = (VaultList.ItemsSource as IEnumerable<VaultItem>)?.FirstOrDefault(item => item.Id == result.Id);
        if (selected is not null) SelectAfterMutation(selected);
    }

    private void RestoreSelection(VaultItem? item)
    {
        if (item is null || _requiresSync || !_loaded) return;
        VaultList.SelectedItem = null;
        var selected = (VaultList.ItemsSource as IEnumerable<VaultItem>)?.FirstOrDefault(candidate => candidate.Id == item.Id);
        if (selected is not null) SelectAfterMutation(selected);
    }

    private void SelectAfterMutation(VaultItem item)
    {
        if (!item.Reprompt) { VaultList.SelectedItem = item; return; }
        // A metadata action such as favorite must not cause a new password prompt as a side effect.
        _syncing = true;
        try { VaultList.SelectedItem = item; }
        finally { _syncing = false; }
        DetailPanel.Children.Clear();
        DetailPanel.Children.Add(new TextBlock { Text = "此条目的详情需要验证主密码后查看。", TextWrapping = TextWrapping.Wrap });
        var view = new Button { Content = "查看详情", HorizontalAlignment = HorizontalAlignment.Left };
        view.Click += (_, _) => { VaultList.SelectedItem = null; VaultList.SelectedItem = item; };
        DetailPanel.Children.Add(view);
        UpdateMutationCommands();
    }

    private void QuickCopyUser_Click(object sender, RoutedEventArgs e)
    {
        if (VaultList.SelectedItem is VaultItem item) _ = QuickCopyFieldAsync(item, "username");
    }

    private void QuickCopyPassword_Click(object sender, RoutedEventArgs e)
    {
        if (VaultList.SelectedItem is VaultItem item) _ = QuickCopyFieldAsync(item, "password");
    }

    private void QuickCopyTotp_Click(object sender, RoutedEventArgs e)
    {
        if (VaultList.SelectedItem is VaultItem item) _ = QuickCopyFieldAsync(item, "totp");
    }

    private void QuickCopyUsernameShortcut_Invoked(Microsoft.UI.Xaml.Input.KeyboardAccelerator sender,
        Microsoft.UI.Xaml.Input.KeyboardAcceleratorInvokedEventArgs args)
    {
        args.Handled = true;
        if (!_selectingItems && VaultList.SelectedItem is VaultItem item) _ = QuickCopyFieldAsync(item, "username");
    }

    private void QuickCopyPasswordShortcut_Invoked(Microsoft.UI.Xaml.Input.KeyboardAccelerator sender,
        Microsoft.UI.Xaml.Input.KeyboardAcceleratorInvokedEventArgs args)
    {
        args.Handled = true;
        if (!_selectingItems && VaultList.SelectedItem is VaultItem item) _ = QuickCopyFieldAsync(item, "password");
    }

    private void QuickCopyTotpShortcut_Invoked(Microsoft.UI.Xaml.Input.KeyboardAccelerator sender,
        Microsoft.UI.Xaml.Input.KeyboardAcceleratorInvokedEventArgs args)
    {
        args.Handled = true;
        if (!_selectingItems && VaultList.SelectedItem is VaultItem item) _ = QuickCopyFieldAsync(item, "totp");
    }

    private void VaultList_ContextRequested(UIElement sender, Microsoft.UI.Xaml.Input.ContextRequestedEventArgs args)
    {
        if (!_loaded || _overview is null || _busy || _syncing || _editorOpen) return;
        var target = (args.OriginalSource as FrameworkElement)?.DataContext as VaultItem ?? VaultList.SelectedItem as VaultItem;
        if (_selectingItems) return;
        if (target is null) return;

        var flyout = new MenuFlyout();
        if (!target.Restricted && target.ItemType == 1)
        {
            var copyUser = new MenuFlyoutItem { Text = "复制用户名", Icon = new SymbolIcon(Symbol.Copy) };
            copyUser.Click += (_, _) => _ = QuickCopyFieldAsync(target, "username");
            var copyPw = new MenuFlyoutItem { Text = "复制密码", Icon = new SymbolIcon(Symbol.Permissions) };
            copyPw.Click += (_, _) => _ = QuickCopyFieldAsync(target, "password");
            var copyTotp = new MenuFlyoutItem { Text = "复制验证码", Icon = new SymbolIcon(Symbol.Clock) };
            copyTotp.Click += (_, _) => _ = QuickCopyFieldAsync(target, "totp");
            flyout.Items.Add(copyUser);
            flyout.Items.Add(copyPw);
            flyout.Items.Add(copyTotp);
            flyout.Items.Add(new MenuFlyoutSeparator());
        }
        if (!_requiresSync)
        {
            if (!target.Deleted && !target.Restricted)
            {
                var edit = new MenuFlyoutItem { Text = "编辑", Icon = new SymbolIcon(Symbol.Edit) };
                edit.Click += (_, _) => _ = OpenEditorAsync(target);
                var clone = new MenuFlyoutItem { Text = "克隆", Icon = new SymbolIcon(Symbol.Copy) };
                clone.Click += (_, _) => _ = OpenEditorAsync(target, clone: true);
                flyout.Items.Add(edit);
                flyout.Items.Add(clone);
            }
            if (!target.Deleted)
            {
                var favorite = new MenuFlyoutItem { Text = target.Favorite ? "取消收藏" : "收藏", Icon = new SymbolIcon(Symbol.Favorite) };
                favorite.Click += (_, _) => _ = ExecuteItemActionAsync(target, "favorite");
                var archive = new MenuFlyoutItem { Text = target.Archived ? "取消归档" : "归档", Icon = new SymbolIcon(Symbol.Folder) };
                archive.Click += (_, _) => _ = ExecuteItemActionAsync(target, "archive");
                flyout.Items.Add(favorite);
                flyout.Items.Add(archive);
            }
            if (!target.Restricted)
            {
                if (!target.Deleted)
                {
                    var trash = new MenuFlyoutItem { Text = "移至回收站", Icon = new SymbolIcon(Symbol.Delete) };
                    trash.Click += (_, _) => _ = ExecuteItemActionAsync(target, "trash");
                    flyout.Items.Add(trash);
                }
                else
                {
                    var restore = new MenuFlyoutItem { Text = "还原", Icon = new SymbolIcon(Symbol.Undo) };
                    restore.Click += (_, _) => _ = ExecuteItemActionAsync(target, "restore");
                    var delete = new MenuFlyoutItem { Text = "永久删除", Icon = new SymbolIcon(Symbol.Delete) };
                    delete.Click += (_, _) => _ = ExecuteItemActionAsync(target, "delete");
                    flyout.Items.Add(restore);
                    flyout.Items.Add(delete);
                }
            }
        }
        if (flyout.Items.Count == 0) return;
        args.Handled = true;
        if (args.TryGetPosition(sender, out var point))
            flyout.ShowAt(sender, new Microsoft.UI.Xaml.Controls.Primitives.FlyoutShowOptions { Position = point });
        else if (sender is FrameworkElement anchor)
            flyout.ShowAt(anchor);
    }

    private async Task QuickCopyFieldAsync(VaultItem item, string kind)
    {
        if (!_loaded || _overview is null || _busy || _syncing || _editorOpen || item.Restricted) return;
        if (item.ItemType != 1 && (kind is "password" or "totp")) return;
        CancelOperation();
        var version = ++_version;
        var cancellation = _operation = new CancellationTokenSource();
        var token = cancellation.Token;
        SetBusy(true);
        var password = string.Empty;
        ulong? leaseId = null;
        try
        {
            if (item.Reprompt)
            {
                var entered = await PromptMasterPasswordAsync();
                if (entered is null || !IsCurrent(version, token)) return;
                password = entered;
            }
            var detail = await Task.Run(() => RustCore.GetVaultDetail(item.Id, password, token), token);
            password = string.Empty;
            leaseId = detail.TotpRequestId ?? detail.AttachmentRequestId;
            if (!IsCurrent(version, token)) return;
            if (kind == "totp")
            {
                if (detail.Totp is { Status: "ready", Value: { Code: { Length: > 0 } code } })
                {
                    ClipboardCleaner.Copy(code);
                    SetNotice("已复制验证码", $"{item.DisplayName} 的验证码已放入剪贴板。", InfoBarSeverity.Success);
                }
                else if (detail.Totp?.Status == "premiumRequired")
                    SetNotice("无法复制验证码", "生成验证码需要个人或组织的高级会员资格。", InfoBarSeverity.Informational);
                else
                    SetNotice("无可用验证码", $"{item.DisplayName} 未配置有效的验证器密钥。", InfoBarSeverity.Informational);
                return;
            }
            var targetLabel = kind == "password" ? "密码" : "用户名";
            var field = detail.Fields.FirstOrDefault(entry => entry.Label == targetLabel);
            if (field is null || string.IsNullOrEmpty(field.Value))
            {
                SetNotice($"无{targetLabel}", $"{item.DisplayName} 未保存{targetLabel}。", InfoBarSeverity.Informational);
                return;
            }
            ClipboardCleaner.Copy(field.Value);
            SetNotice($"已复制{targetLabel}", $"{item.DisplayName} 的{targetLabel}已放入剪贴板。", InfoBarSeverity.Success);
        }
        catch (OperationCanceledException) { }
        catch (VaultException error) { if (IsCurrent(version, token)) SetNotice("快捷复制失败", error.UserMessage, InfoBarSeverity.Error); }
        catch (Exception error) when (error is System.Runtime.InteropServices.COMException or InvalidOperationException)
        { if (IsCurrent(version, token)) SetNotice("剪贴板不可用", "请稍后再试。", InfoBarSeverity.Error); }
        catch (Exception error) when (IsCoreUnavailable(error))
        { if (IsCurrent(version, token)) SetNotice("核心不可用", "请重新构建或修复应用安装。", InfoBarSeverity.Error); }
        finally
        {
            password = string.Empty;
            if (leaseId is { } id)
            {
                try { RustCore.CloseVaultDetail(id); } catch { }
            }
            if (ReferenceEquals(_operation, cancellation)) _operation = null;
            cancellation.Dispose();
            if (version == _version) SetBusy(false);
        }
    }
}
