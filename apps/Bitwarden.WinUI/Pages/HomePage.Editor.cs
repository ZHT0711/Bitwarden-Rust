using Bitwarden_WinUI.Interop;
using Bitwarden_WinUI.Controls;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace Bitwarden_WinUI.Pages;

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
        var canMove = available && _category != "trash";
        SelectItemsButton.IsEnabled = canMove;
        SelectItemsButton.Label = _selectingItems ? "退出批量选择" : "批量选择";
        MoveItemsButton.IsEnabled = canMove && VaultList.SelectedItems.Count > 0;
        available &= !_selectingItems;
        var item = VaultList.SelectedItem as VaultItem;
        NewItemButton.IsEnabled = available;
        EditItemButton.IsEnabled = available && item is { Deleted: false, Restricted: false };
        FavoriteItemButton.IsEnabled = ArchiveItemButton.IsEnabled = available && item is { Deleted: false };
        TrashItemButton.IsEnabled = available && item is { Deleted: false, Restricted: false };
        RestoreItemButton.IsEnabled = DeleteItemButton.IsEnabled = available && item is { Deleted: true, Restricted: false };
        var deleted = item?.Deleted == true;
        FavoriteItemButton.Visibility = ArchiveItemButton.Visibility = TrashItemButton.Visibility = deleted ? Visibility.Collapsed : Visibility.Visible;
        RestoreItemButton.Visibility = DeleteItemButton.Visibility = deleted ? Visibility.Visible : Visibility.Collapsed;
        FavoriteItemButton.Label = item?.Favorite == true ? "取消收藏" : "收藏";
        ArchiveItemButton.Label = item?.Archived == true ? "取消归档" : "归档";
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

    private void SelectItems_Click(object sender, RoutedEventArgs e)
    {
        if (!SelectItemsButton.IsEnabled) return;
        CancelOperation();
        _version++;
        SetItemSelectionMode(!_selectingItems);
        DetailPanel.Children.Clear();
        DetailPanel.Children.Add(new TextBlock
        {
            Text = _selectingItems ? "选择要移动的条目" : "选择一个条目查看详情",
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

        var folders = new ComboBox { Header = "目标文件夹", MinWidth = 240 };
        folders.Items.Add(new ComboBoxItem { Content = "无文件夹" });
        foreach (var folder in _overview.Folders)
            folders.Items.Add(new ComboBoxItem { Content = folder.Name, Tag = folder.Id });
        folders.SelectedIndex = 0;
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
            var target = (folders.SelectedItem as ComboBoxItem)?.Tag as string;
            folders.Items.Clear();
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
            folders.Items.Clear();
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

    private async Task OpenEditorAsync(VaultItem? item)
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
            }
            _activeEditor = VaultEditorDialog.Create(XamlRoot, data, _overview.Folders, async (draft, requestToken) =>
            {
                try
                {
                    return await Task.Run(() => item is null ? RustCore.CreateVaultItem(draft, requestToken)
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
        if (result is not null) ApplyMutationResult(result, "条目已保存");
        else if (_requiresSync) SetNotice("需要同步核对", "条目可能已在服务器更新。请先同步，再进行其他写入。", InfoBarSeverity.Warning);
        else RestoreSelection(item ?? previousSelection);
    }

    private async void ItemAction_Click(object sender, RoutedEventArgs e)
    {
        if (sender is not AppBarButton { Tag: string requested } || VaultList.SelectedItem is not VaultItem item ||
            _editorOpen || _syncing || _busy || _requiresSync || _overview is null) return;
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
}
