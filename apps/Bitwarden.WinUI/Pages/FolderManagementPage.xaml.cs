using Bitwarden_WinUI.Interop;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace Bitwarden_WinUI.Pages;

public sealed partial class FolderManagementPage : Page
{
    public event EventHandler? SignInRequested;

    private FolderRow[]? _folders;
    private CancellationTokenSource? _operation;
    private ContentDialog? _activeDialog;
    private string _query = string.Empty;
    private string _emptyStateMessage = "登录以查看文件夹";
    private bool _loaded;
    private bool _busy;
    private bool _requiresSync;

    private bool CanWrite => _loaded && _folders is not null && !_busy && !_requiresSync && _activeDialog is null;

    public FolderManagementPage()
    {
        InitializeComponent();
        Loaded += async (_, _) =>
        {
            _loaded = true;
            await RefreshAsync(false);
        };
        Unloaded += (_, _) =>
        {
            _loaded = false;
            ClearSensitiveState();
        };
    }

    public void SetSearch(string query)
    {
        _query = query.Trim();
        RenderFolders();
    }

    public void ClearSensitiveState()
    {
        _operation?.Cancel();
        _operation = null;
        var dialog = _activeDialog;
        _activeDialog = null;
        if (dialog?.Content is TextBox input) input.Text = string.Empty;
        dialog?.Hide();
        _folders = null;
        _query = string.Empty;
        _requiresSync = false;
        _emptyStateMessage = "登录以查看文件夹";
        ConnectButton.Visibility = Visibility.Collapsed;
        ConnectionNotice.IsOpen = false;
        SetBusy(false);
        RenderFolders();
    }

    public async Task RefreshAsync(bool forceSync)
    {
        if (!_loaded || _busy || _activeDialog is not null) return;
        using var operation = _operation = new CancellationTokenSource();
        var token = operation.Token;
        try
        {
            var session = RustCore.SessionState;
            if (session != VaultSessionState.Unlocked)
            {
                _folders = null;
                var locked = session == VaultSessionState.Locked;
                _emptyStateMessage = locked ? "保险库已锁定" : "登录以查看文件夹";
                ConnectButton.Content = locked ? "解锁保险库" : "连接账户";
                ConnectButton.Visibility = Visibility.Visible;
                ConnectionNotice.IsOpen = false;
                RenderFolders();
                return;
            }

            ConnectButton.Visibility = Visibility.Collapsed;
            _emptyStateMessage = "同步保险库以查看文件夹";
            SetBusy(true);
            ConnectionNotice.IsOpen = false;
            VaultOverview overview;
            try
            {
                overview = forceSync ? await Task.Run(() => RustCore.SyncVault(token), token)
                    : await Task.Run(RustCore.GetVaultOverview, token);
            }
            catch (VaultException error) when (!forceSync && (error.Code is "notFound" or "notSynced"))
            {
                _requiresSync = true;
                overview = await Task.Run(() => RustCore.SyncVault(token), token);
            }

            if (!ReferenceEquals(_operation, operation)) return;
            _requiresSync = false;
            ApplyOverview(overview);
        }
        catch (OperationCanceledException) { }
        catch (VaultException error)
        {
            if (ReferenceEquals(_operation, operation))
            {
                UpdateSyncRequirement(error);
                SetNotice("无法加载文件夹", error.UserMessage, InfoBarSeverity.Error);
                RenderFolders();
            }
        }
        catch (Exception error) when (IsCoreUnavailable(error))
        {
            if (ReferenceEquals(_operation, operation)) ShowCoreUnavailable();
        }
        finally
        {
            FinishOperation(operation);
        }
    }

    private void ApplyOverview(VaultOverview overview)
    {
        var counts = new Dictionary<string, int>();
        foreach (var item in overview.Items)
        {
            if (item.FolderId is { } id && !item.Deleted)
                counts[id] = counts.GetValueOrDefault(id) + 1;
        }
        _folders = overview.Folders
            .Select(folder => new FolderRow(folder.Id, folder.Name, counts.GetValueOrDefault(folder.Id)))
            .ToArray();
        RenderFolders();
    }

    private void RenderFolders()
    {
        var rows = _folders is null ? [] : _folders
            .Where(folder => folder.Name.Contains(_query, StringComparison.OrdinalIgnoreCase))
            .ToArray();
        FolderList.ItemsSource = rows;
        FolderCount.Text = $"{rows.Length} 个文件夹";
        EmptyState.Visibility = rows.Length == 0 ? Visibility.Visible : Visibility.Collapsed;
        EmptyMessage.Text = _folders is null ? _emptyStateMessage : _query.Length > 0 ? "没有匹配的文件夹" : "还没有文件夹";
        UpdateCommands();
    }

    private async void CreateFolder_Click(object sender, RoutedEventArgs e)
    {
        var name = await ShowFolderNameDialogAsync("新建文件夹", string.Empty);
        if (name is null) return;
        await RunMutationAsync(token => RustCore.CreateFolder(name, token), "文件夹已创建");
    }

    private async void RenameSelected_Click(object sender, RoutedEventArgs e)
    {
        if (CanWrite && FolderList.SelectedItems.Count == 1 && FolderList.SelectedItems[0] is FolderRow folder)
            await RenameFolderAsync(folder);
    }

    private async void RenameRow_Click(object sender, RoutedEventArgs e)
    {
        if ((sender as Button)?.Tag is FolderRow folder) await RenameFolderAsync(folder);
    }

    private async Task RenameFolderAsync(FolderRow folder)
    {
        var name = await ShowFolderNameDialogAsync("重命名文件夹", folder.Name);
        if (name is null || name == folder.Name) return;
        await RunMutationAsync(token => RustCore.RenameFolder(folder.Id, name, token), "文件夹已重命名");
    }

    private async void DeleteRow_Click(object sender, RoutedEventArgs e)
    {
        if ((sender as Button)?.Tag is FolderRow folder) await DeleteFoldersAsync([folder]);
    }

    private async void DeleteSelected_Click(object sender, RoutedEventArgs e)
    {
        await DeleteFoldersAsync(FolderList.SelectedItems.OfType<FolderRow>().ToArray());
    }

    private async Task DeleteFoldersAsync(FolderRow[] folders)
    {
        if (!CanWrite || folders.Length == 0) return;
        if (folders.Length > 500)
        {
            SetNotice("所选文件夹过多", "每次最多可删除 500 个文件夹。", InfoBarSeverity.Warning);
            return;
        }

        var dialog = new ContentDialog
        {
            XamlRoot = XamlRoot,
            Title = folders.Length == 1 ? "删除文件夹？" : $"删除 {folders.Length} 个文件夹？",
            Content = new TextBlock
            {
                Text = (folders.Length == 1 ? $"删除“{folders[0].Name}”？\n" : string.Empty)
                    + "文件夹中的条目会保留，并移至“无文件夹”。",
                TextWrapping = TextWrapping.Wrap,
            },
            PrimaryButtonText = "删除",
            CloseButtonText = "取消",
            DefaultButton = ContentDialogButton.Close,
        };
        if (!await ConfirmAsync(dialog)) return;
        var ids = folders.Select(folder => folder.Id).ToArray();
        await RunMutationAsync(token => RustCore.DeleteFolders(ids, token), $"已删除 {ids.Length} 个文件夹");
    }

    private async Task RunMutationAsync(Func<CancellationToken, VaultMutationResult> write, string successTitle)
    {
        if (!CanWrite) return;
        using var operation = _operation = new CancellationTokenSource();
        var token = operation.Token;
        SetBusy(true);
        ConnectionNotice.IsOpen = false;
        try
        {
            var result = await Task.Run(() => write(token), token);
            if (!ReferenceEquals(_operation, operation)) return;
            ApplyOverview(result.Overview);
            SetNotice(successTitle, string.Empty, InfoBarSeverity.Success);
        }
        catch (OperationCanceledException)
        {
            if (ReferenceEquals(_operation, operation))
            {
                _requiresSync = true;
                SetNotice("请求已取消", "请同步以核对服务器状态。", InfoBarSeverity.Warning);
            }
        }
        catch (VaultException error)
        {
            if (ReferenceEquals(_operation, operation))
            {
                UpdateSyncRequirement(error);
                var message = error.Code switch
                {
                    "notFound" => "找不到该文件夹。请先同步保险库。",
                    "validation" => "服务器拒绝了文件夹更改，请检查名称或所选文件夹。",
                    _ => error.UserMessage,
                };
                SetNotice("无法更新文件夹", message, InfoBarSeverity.Error);
            }
        }
        catch (Exception error) when (IsCoreUnavailable(error))
        {
            if (ReferenceEquals(_operation, operation)) ShowCoreUnavailable();
        }
        finally
        {
            FinishOperation(operation);
        }
    }

    private async Task<string?> ShowFolderNameDialogAsync(string title, string initialValue)
    {
        if (!CanWrite) return null;
        var input = new TextBox { Header = "文件夹名称", Text = initialValue, MinWidth = 240 };
        var dialog = new ContentDialog
        {
            XamlRoot = XamlRoot,
            Title = title,
            Content = input,
            PrimaryButtonText = "保存",
            CloseButtonText = "取消",
            DefaultButton = ContentDialogButton.Primary,
            IsPrimaryButtonEnabled = !string.IsNullOrWhiteSpace(initialValue),
        };
        input.TextChanged += (_, _) => dialog.IsPrimaryButtonEnabled = !string.IsNullOrWhiteSpace(input.Text);
        dialog.Opened += (_, _) => input.Focus(FocusState.Programmatic);
        try { return await ConfirmAsync(dialog) ? input.Text.Trim() : null; }
        finally { input.Text = string.Empty; }
    }

    private async Task<bool> ConfirmAsync(ContentDialog dialog)
    {
        _activeDialog = dialog;
        try
        {
            var result = await dialog.ShowAsync();
            return ReferenceEquals(_activeDialog, dialog) && result == ContentDialogResult.Primary;
        }
        finally
        {
            if (ReferenceEquals(_activeDialog, dialog)) _activeDialog = null;
        }
    }

    private void FolderList_SelectionChanged(object sender, SelectionChangedEventArgs e) => UpdateCommands();

    private void ConnectAccount_Click(object sender, RoutedEventArgs e) => SignInRequested?.Invoke(this, EventArgs.Empty);

    private void UpdateCommands()
    {
        var selected = FolderList.SelectedItems.Count;
        CreateFolderButton.IsEnabled = CanWrite;
        RenameFolderButton.IsEnabled = CanWrite && selected == 1;
        DeleteFoldersButton.IsEnabled = CanWrite && selected > 0;
        FolderList.IsEnabled = !_busy && !_requiresSync;
    }

    private void UpdateSyncRequirement(VaultException error)
    {
        _requiresSync |= error.Code is "conflict" or "writeUncertain" or "notSynced" or "notFound";
    }

    private void FinishOperation(CancellationTokenSource operation)
    {
        if (!ReferenceEquals(_operation, operation)) return;
        _operation = null;
        SetBusy(false);
    }

    private void SetBusy(bool busy)
    {
        _busy = busy;
        LoadingRing.IsActive = busy;
        UpdateCommands();
    }

    private void SetNotice(string title, string message, InfoBarSeverity severity)
    {
        ConnectionNotice.Title = title;
        ConnectionNotice.Message = message;
        ConnectionNotice.Severity = severity;
        ConnectionNotice.IsOpen = true;
    }

    private void ShowCoreUnavailable() =>
        SetNotice("核心不可用", "请重新构建或修复应用安装。", InfoBarSeverity.Error);

    private static bool IsCoreUnavailable(Exception error) =>
        error is DllNotFoundException or EntryPointNotFoundException or BadImageFormatException;

    private sealed record FolderRow(string Id, string Name, int ItemCount)
    {
        public string ItemCountLabel => $"{ItemCount} 个条目";
        public override string ToString() => Name;
    }
}
