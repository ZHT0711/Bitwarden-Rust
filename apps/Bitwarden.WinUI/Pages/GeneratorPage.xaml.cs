using Bitwarden_WinUI.Interop;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Windows.ApplicationModel.DataTransfer;

namespace Bitwarden_WinUI.Pages;

public sealed partial class GeneratorPage : Page
{
    public GeneratorPage()
    {
        InitializeComponent();
    }

    private void LengthSlider_ValueChanged(object sender, RangeBaseValueChangedEventArgs e)
    {
        if (LengthLabel is not null)
        {
            LengthLabel.Text = ((int)e.NewValue).ToString();
        }
    }

    private void Generate_Click(object sender, RoutedEventArgs e)
    {
        try
        {
            GeneratedPassword.Text = RustCore.GeneratePassword(
                (uint)LengthSlider.Value,
                Uppercase.IsOn,
                Lowercase.IsOn,
                Numbers.IsOn,
                Special.IsOn,
                AvoidAmbiguous.IsOn,
                ToCount(MinimumNumbers.Value),
                ToCount(MinimumSpecial.Value));
            GeneratorNotice.Title = "已生成";
            GeneratorNotice.Severity = InfoBarSeverity.Success;
            GeneratorNotice.IsOpen = false;
        }
        catch (Exception error) when (error is DllNotFoundException or EntryPointNotFoundException or BadImageFormatException or InvalidOperationException)
        {
            GeneratorNotice.Title = "无法生成密码";
            GeneratorNotice.Severity = InfoBarSeverity.Error;
            GeneratorNotice.Message = error.Message.Contains("invalid", StringComparison.OrdinalIgnoreCase)
                ? "当前选项无法在所选长度内满足，请增加长度或减少最少字符数。"
                : "Rust 核心 DLL 未加载；请从仓库根目录构建应用。";
            GeneratorNotice.IsOpen = true;
        }
    }

    private void Clear_Click(object sender, RoutedEventArgs e)
    {
        GeneratedPassword.Text = string.Empty;
        GeneratorNotice.Title = "无法生成密码";
        GeneratorNotice.Severity = InfoBarSeverity.Error;
        GeneratorNotice.IsOpen = false;
    }

    private void Copy_Click(object sender, RoutedEventArgs e)
    {
        if (string.IsNullOrEmpty(GeneratedPassword.Text))
        {
            GeneratorNotice.Title = "没有可复制的密码";
            GeneratorNotice.Message = "请先生成密码。";
            GeneratorNotice.Severity = InfoBarSeverity.Warning;
            GeneratorNotice.IsOpen = true;
            return;
        }

        var package = new DataPackage();
        package.SetText(GeneratedPassword.Text);
        Clipboard.SetContent(package);
        GeneratorNotice.Title = "已复制";
        GeneratorNotice.Message = "密码已复制到剪贴板。自动清理将在后续迭代接入。";
        GeneratorNotice.Severity = InfoBarSeverity.Success;
        GeneratorNotice.IsOpen = true;
    }

    private static uint ToCount(double? value) => value is double number && number > 0 ? (uint)number : 0;
}
