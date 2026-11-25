using Bitwarden_WinUI.Interop;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace Bitwarden_WinUI.Pages;

public sealed partial class HomePage
{
    private VaultItem[]? _orderedSource;
    private VaultItem[] _orderedItems = [];
    private VaultSort _sort;
    private bool _updatingList;
    private bool _compactList;
    private bool _listOptionsReady;

    private void InitializeListOptions()
    {
        foreach (var label in new[] { "名称（A → Z）", "名称（Z → A）", "最近修改优先", "最早修改优先" })
            SortChoice.Items.Add(label);
        _sort = (VaultSort)DesktopPreferences.ReadVaultSort();
        SortChoice.SelectedIndex = (int)_sort;
        _listOptionsReady = true;
    }

    private VaultItem[] OrderedItems()
    {
        var source = _overview?.Items;
        if (!ReferenceEquals(source, _orderedSource))
        {
            _orderedSource = source;
            _orderedItems = VaultQuery.Sort(source ?? [], _sort);
        }
        return _orderedItems;
    }

    private void SortChoice_Changed(object sender, SelectionChangedEventArgs e)
    {
        if (!_listOptionsReady || SortChoice.SelectedIndex < 0) return;
        _sort = (VaultSort)SortChoice.SelectedIndex;
        SortFeedback.Text = DesktopPreferences.TrySaveVaultSort((int)_sort) ? string.Empty : "本次排序已应用，但无法保存到本机。";
        SortFeedback.Visibility = SortFeedback.Text.Length == 0 ? Visibility.Collapsed : Visibility.Visible;
        _orderedSource = null;
        ResetFilterCache();
        ApplyFilters();
    }

    private bool UpdateListItems(VaultItem[] items)
    {
        // A no-op search must not clear selection or trigger another reprompt / detail decryption.
        var previous = VaultList.ItemsSource as VaultItem[];
        if (VaultQuery.SameProjection(previous, items)) return false;

        VaultItem[] selected;
        if (_selectingItems)
        {
            selected = VaultList.SelectedItems.Count == 0 ? [] : VaultList.SelectedItems.OfType<VaultItem>().ToArray();
        }
        else
        {
            selected = VaultList.SelectedItem is VaultItem current ? [current] : [];
        }

        var retained = VaultQuery.RetainSelection(selected, items);
        _updatingList = true;
        try
        {
            VaultList.ItemsSource = items;
            if (_selectingItems)
                foreach (var item in retained) VaultList.SelectedItems.Add(item);
            else if (retained.Length == 1) VaultList.SelectedItem = retained[0];
        }
        finally { _updatingList = false; }
        if (selected.Length > 0 && retained.Length == 0 && !_syncing && !_editorOpen)
        {
            CancelOperation();
            _version++;
            ClearDetailInputs(DetailPanel);
            DetailPanel.Children.Clear();
            SetBusy(false);
        }
        UpdateListLayout();
        return true;
    }

    private void FolderChoiceLabel_Loaded(object sender, RoutedEventArgs e)
    {
        if (sender is FrameworkElement label) Controls.FolderPicker.SetContainerIdentity(label);
    }

    private void FolderChoiceLabel_DataContextChanged(FrameworkElement sender, DataContextChangedEventArgs e) =>
        Controls.FolderPicker.SetContainerIdentity(sender);

    private void ListPane_SizeChanged(object sender, SizeChangedEventArgs e)
    {
        // The decoration must not displace the status or account action in a short viewport.
        // The text/action remain scrollable rather than being arranged outside the visible pane.
        EmptyIcon.Visibility = e.NewSize.Height < 160 ? Visibility.Collapsed : Visibility.Visible;
    }

    private void VaultLayout_SizeChanged(object sender, SizeChangedEventArgs e)
    {
        // Bound the header, not the virtualized item list. All filters/ancestors remain
        // reachable by scrolling, even when their desired height exceeds the window.
        VaultHeaderScroll.MaxHeight = Math.Max(0, e.NewSize.Height - VaultLayout.Padding.Top -
            VaultLayout.Padding.Bottom - VaultLayout.RowSpacing - 96);
        _compactList = e.NewSize.Width < 720;
        VisualStateManager.GoToState(this, _compactList ? "CompactVault" : "WideVault", false);
        UpdateListLayout();
    }

    private void UpdateListLayout()
    {
        if (ListPane is null || DetailPane is null) return;
        var showDetail = _compactList && !_selectingItems && VaultList.SelectedItem is VaultItem;
        ListPane.Visibility = showDetail ? Visibility.Collapsed : Visibility.Visible;
        DetailPane.Visibility = !_compactList || showDetail ? Visibility.Visible : Visibility.Collapsed;
        DetailBackButton.Visibility = showDetail ? Visibility.Visible : Visibility.Collapsed;
    }

    private void DetailBack_Click(object sender, RoutedEventArgs e)
    {
        if (_editorOpen) return;
        VaultList.SelectedItem = null;
        UpdateListLayout();
        VaultList.Focus(FocusState.Programmatic);
    }
}
