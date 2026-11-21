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
        if (detail.Attachments == 0) return;

        var section = new StackPanel { Spacing = 8 };
        section.Children.Add(new TextBlock
        {
            Text = "附件",
            Style = (Style)Application.Current.Resources["BodyStrongTextBlockStyle"],
        });
        foreach (var attachment in detail.AttachmentFiles ?? [])
        {
            var row = new Grid { ColumnSpacing = 8 };
            row.ColumnDefinitions.Add(new ColumnDefinition());
            row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            var label = new StackPanel { VerticalAlignment = VerticalAlignment.Center };
            label.Children.Add(new TextBlock { Text = attachment.Name, TextWrapping = TextWrapping.Wrap });
            if (attachment.Size is { } size)
                label.Children.Add(new TextBlock
                {
                    Text = $"{size:N0} B",
                    Style = (Style)Application.Current.Resources["CaptionTextBlockStyle"],
                });
            row.Children.Add(label);
            var download = new Button
            {
                Content = "下载",
                IsEnabled = requestId.HasValue,
                VerticalAlignment = VerticalAlignment.Center,
            };
            AutomationProperties.SetName(download, $"下载 {attachment.Name}");
            download.Click += async (_, _) =>
            {
                if (requestId is { } activeId) await DownloadAttachmentAsync(attachment, activeId);
            };
            Grid.SetColumn(download, 1);
            row.Children.Add(download);
            section.Children.Add(row);
        }
        if (detail.AttachmentFailures > 0)
            section.Children.Add(new InfoBar
            {
                IsOpen = true,
                Severity = InfoBarSeverity.Warning,
                Message = $"{detail.AttachmentFailures} 个附件的文件名或密钥无法读取。",
            });
        DetailPanel.Children.Add(section);
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
