using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;

namespace Bitwarden_WinUI.Controls;

internal static class PasswordVisibility
{
    internal static Grid WithToggle(PasswordBox box)
    {
        box.PasswordRevealMode = PasswordRevealMode.Hidden;
        var label = box.Header as string ?? "密码";
        var icon = new SymbolIcon(Symbol.View);
        var toggle = new ToggleButton
        {
            Content = icon,
            MinWidth = 36,
            VerticalAlignment = VerticalAlignment.Bottom,
        };
        AutomationProperties.SetAutomationId(toggle, "RevealPassword");

        void UpdateMode()
        {
            var visible = toggle.IsChecked == true;
            box.PasswordRevealMode = visible ? PasswordRevealMode.Visible : PasswordRevealMode.Hidden;
            var action = (visible ? "隐藏" : "显示") + label;
            AutomationProperties.SetName(toggle, action);
            ToolTipService.SetToolTip(toggle, action);
        }

        void UpdateAvailability()
        {
            toggle.IsEnabled = box.IsEnabled && box.Password.Length != 0;
            if (!toggle.IsEnabled) toggle.IsChecked = false;
        }

        toggle.Checked += (_, _) => UpdateMode();
        toggle.Unchecked += (_, _) => UpdateMode();
        box.PasswordChanged += (_, _) => UpdateAvailability();
        box.IsEnabledChanged += (_, _) => UpdateAvailability();
        box.Unloaded += (_, _) => toggle.IsChecked = false;
        UpdateAvailability();
        UpdateMode();

        var row = new Grid { ColumnSpacing = 6 };
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        row.Children.Add(box);
        Grid.SetColumn(toggle, 1);
        row.Children.Add(toggle);
        return row;
    }
}
