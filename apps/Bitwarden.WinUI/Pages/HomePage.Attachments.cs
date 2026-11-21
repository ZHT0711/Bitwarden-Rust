using System.Runtime.InteropServices;
using Bitwarden_WinUI.Interop;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.Windows.Storage.Pickers;

namespace Bitwarden_WinUI.Pages;

public sealed partial class HomePage
{
    private ulong? _attachmentRequestId;

    private void ShowAttachmentDetails(VaultDetail detail)
    {
        var requestId = _attachmentRequestId = detail.AttachmentRequestId;
        if (detail.Attachments == 0 && !detail.CanUploadAttachments) return;

        var section = new StackPanel { Spacing = 8 };
        section.Children.Add(new TextBlock
        {
            Text = "附件",
            Style = (Style)Application.Current.Resources["BodyStrongTextBlockStyle"],
        });
        if (requestId is { } uploadRequest && detail.CanUploadAttachments)
        {
            var upload = new Button { Content = "添加附件", HorizontalAlignment = HorizontalAlignment.Left };
            upload.Click += async (_, _) => await RunAttachmentWriteAsync(uploadRequest, PickAttachmentAsync,
                RustCore.UploadVaultAttachment, "正在上传附件", "附件已上传");
            section.Children.Add(upload);
        }
        foreach (var attachment in detail.AttachmentFiles ?? [])
        {
            var row = new Grid { ColumnSpacing = 8 };
            row.ColumnDefinitions.Add(new ColumnDefinition());
            row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            var label = new StackPanel { VerticalAlignment = VerticalAlignment.Center };
            var name = string.IsNullOrEmpty(attachment.Name) ? "无法解密的附件" : attachment.Name;
            label.Children.Add(new TextBlock { Text = name, TextWrapping = TextWrapping.Wrap });
            if (attachment.Size is { } size)
                label.Children.Add(new TextBlock
                {
                    Text = $"{size:N0} B",
                    Style = (Style)Application.Current.Resources["CaptionTextBlockStyle"],
                });
            row.Children.Add(label);
            var actions = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 6 };
            if (requestId is { } activeId && attachment.CanDownload)
            {
                var download = new Button { Content = "下载", VerticalAlignment = VerticalAlignment.Center };
                AutomationProperties.SetName(download, $"下载 {name}");
                download.Click += async (_, _) => await DownloadAttachmentAsync(attachment, activeId);
                actions.Children.Add(download);
            }
            if (requestId is { } deleteRequest && detail.CanDeleteAttachments)
            {
                var delete = new Button { Content = "删除", VerticalAlignment = VerticalAlignment.Center };
                AutomationProperties.SetName(delete, $"删除 {name}");
                delete.Click += async (_, _) => await RunAttachmentWriteAsync(deleteRequest,
                    _ => ConfirmAttachmentDeletionAsync(attachment.Id, name), RustCore.DeleteVaultAttachment,
                    "正在删除附件", "附件已删除");
                actions.Children.Add(delete);
            }
            Grid.SetColumn(actions, 1);
            row.Children.Add(actions);
            section.Children.Add(row);
        }
        if (detail.AttachmentFailures > 0)
            section.Children.Add(new InfoBar
            {
                IsOpen = true,
                Severity = InfoBarSeverity.Warning,
                Message = $"{detail.AttachmentFailures} 个附件的标识无法读取。",
            });
        DetailPanel.Children.Add(section);
    }

    private async Task<string?> PickAttachmentAsync(CancellationToken token)
    {
        var picker = new FileOpenPicker(XamlRoot.ContentIslandEnvironment.AppWindowId);
        picker.FileTypeFilter.Add("*");
        return (await picker.PickSingleFileAsync().AsTask(token))?.Path;
    }

    private async Task<string?> ConfirmAttachmentDeletionAsync(string id, string name)
    {
        var dialog = _actionDialog = new ContentDialog
        {
            XamlRoot = XamlRoot,
            Title = "删除附件？",
            Content = new TextBlock { Text = $"将永久删除“{name}”。", TextWrapping = TextWrapping.Wrap },
            PrimaryButtonText = "删除",
            CloseButtonText = "取消",
            DefaultButton = ContentDialogButton.Close,
        };
        try { return await dialog.ShowAsync() == ContentDialogResult.Primary ? id : null; }
        finally { if (ReferenceEquals(_actionDialog, dialog)) _actionDialog = null; }
    }

    private async Task RunAttachmentWriteAsync(ulong requestId, Func<CancellationToken, Task<string?>> select,
        Func<string, ulong, CancellationToken, VaultMutationResult> write, string pendingTitle, string successTitle)
    {
        if (_busy || _requiresSync || _attachmentRequestId != requestId || _operation is null) return;
        var version = _version;
        var token = _operation.Token;
        SetBusy(true);
        try
        {
            var value = await select(token);
            if (value is null || !IsCurrent(version, token)) return;
            SetNotice(pendingTitle, string.Empty, InfoBarSeverity.Informational);
            var result = await Task.Run(() => write(value, requestId, token), token);
            if (!_loaded || version != _version) return;
            CancelOperation();
            DetailPanel.Children.Clear();
            ApplyMutationResult(result, successTitle);
        }
        catch (OperationCanceledException)
        {
            if (_loaded && version == _version)
                SetNotice("附件操作已取消", "可重新打开条目后重试。", InfoBarSeverity.Informational);
        }
        catch (VaultException error)
        {
            if (_loaded && version == _version)
            {
                _requiresSync |= error.Code is "conflict" or "writeUncertain" or "notSynced";
                var message = error.Code switch
                {
                    "conflict" => "条目已更新，请先同步保险库。",
                    "writeUncertain" => "服务器上的附件状态待确认，请先同步保险库。",
                    "premiumRequired" => "上传附件需要个人或组织提供的高级会员资格。",
                    _ => error.UserMessage,
                };
                SetNotice("附件操作失败", message, InfoBarSeverity.Error);
            }
        }
        catch (Exception error) when (error is COMException or ArgumentException or InvalidOperationException || IsCoreUnavailable(error))
        {
            if (IsCurrent(version, token))
                SetNotice("无法操作附件", "请关闭其他文件对话框，或检查应用安装后重试。", InfoBarSeverity.Error);
        }
        finally
        {
            if (version == _version) SetBusy(false);
        }
    }

    private async Task DownloadAttachmentAsync(VaultAttachment attachment, ulong requestId)
    {
        if (_busy || _attachmentRequestId != requestId || _operation is null) return;
        var version = _version;
        var token = _operation.Token;
        SetBusy(true);
        try
        {
            var invalid = Path.GetInvalidFileNameChars();
            var name = new string(attachment.Name.Select(c => invalid.Contains(c) ? '_' : c).ToArray()).Trim(' ', '.');
            var picker = new FileSavePicker(XamlRoot.ContentIslandEnvironment.AppWindowId)
            {
                SuggestedFileName = name.Length == 0 ? "附件" : name,
                SuggestedStartLocation = PickerLocationId.Downloads,
                ShowOverwritePrompt = true,
            };
            var file = await picker.PickSaveFileAsync().AsTask(token);
            if (file is null || !IsCurrent(version, token)) return;

            SetNotice("正在下载附件", attachment.Name, InfoBarSeverity.Informational);
            await Task.Run(() => RustCore.DownloadVaultAttachment(attachment.Id, file.Path, requestId, token), token);
            if (_loaded && version == _version)
                SetNotice("附件已保存", Path.GetFileName(file.Path), InfoBarSeverity.Success);
        }
        catch (OperationCanceledException)
        {
            if (_loaded && version == _version)
                SetNotice("下载已取消", "可重新打开条目后重试。", InfoBarSeverity.Informational);
        }
        catch (VaultException error)
        {
            if (!IsCurrent(version, token)) return;
            var message = error.Code switch
            {
                "conflict" => "条目已更新，请重新打开后再下载。",
                "decryptionFailed" => "附件无法解密。请同步保险库并重试。",
                _ => error.UserMessage,
            };
            SetNotice("无法下载附件", message, InfoBarSeverity.Error);
        }
        catch (Exception error) when (error is COMException or ArgumentException or InvalidOperationException)
        {
            if (IsCurrent(version, token))
                SetNotice("无法选择保存位置", "请关闭其他文件对话框后重试。", InfoBarSeverity.Error);
        }
        catch (Exception error) when (IsCoreUnavailable(error))
        {
            if (IsCurrent(version, token)) SetNotice("核心不可用", "请修复应用安装后重试。", InfoBarSeverity.Error);
        }
        finally
        {
            if (version == _version) SetBusy(false);
        }
    }
}
