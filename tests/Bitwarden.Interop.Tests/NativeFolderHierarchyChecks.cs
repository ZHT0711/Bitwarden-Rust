using Bitwarden_WinUI.Interop;

internal static class NativeFolderHierarchyChecks
{
    internal static async Task RunAsync(Uri fixture, Action<bool, string> check)
    {
        if (!fixture.IsLoopback || fixture.Scheme != "http") throw new ArgumentException("Only a loopback synthetic fixture is allowed.");
        using var http = new HttpClient { BaseAddress = fixture, Timeout = TimeSpan.FromSeconds(5) };
        var token = CancellationToken.None;
        try
        {
            await Reset();
            var initial = RustCore.SyncVault(token);
            var login = initial.Items.Single(item => item.Name == "Synthetic login");
            var root = RustCore.CreateFolder("Nested parent", token).Id!;
            var child = RustCore.CreateFolder("Nested parent/Child", token).Id!;
            var grand = RustCore.CreateFolder("Nested parent/Child/Leaf", token).Id!;
            var overview = RustCore.MoveVaultItems([login.Id], child, token).Overview;
            var tree = FolderHierarchy.Build(overview.Folders, overview.Items);
            check(tree.Find(root)!.Children.Single().Id == child && tree.Find(child)!.Children.Single().Id == grand,
                "actual DLL creates independent encrypted folder IDs that form three navigation levels");
            check(tree.Find(root) is { ItemCount: 1, DirectItemCount: 0 } && tree.Find(child) is { ItemCount: 1, DirectItemCount: 1 },
                "actual DLL folder membership produces descendant counts without changing parent membership");
            check(VaultQuery.Apply(overview.Items, "vault", root, "").Length == 0 &&
                VaultQuery.Apply(overview.Items, "vault", child, "").Single().Id == login.Id,
                "actual DLL parent and child item filtering remain exact-ID projections");
            var editor = RustCore.GetVaultEditor(login.Id, "", token);
            check(editor.Passkeys?.Length == 1 && editor.FolderId == child, "moving into a nested folder preserves the encrypted login and passkey");
            overview = RustCore.SyncVault(token);
            check(overview.Folders.Single(folder => folder.Id == child).Name == "Nested parent/Child" &&
                overview.Items.Single(item => item.Id == login.Id).FolderId == child,
                "nested full names and item assignments survive a server sync");
            overview = RustCore.RenameFolder(root, "Renamed parent", token).Overview;
            tree = FolderHierarchy.Build(overview.Folders, overview.Items);
            check(overview.Folders.Single(folder => folder.Id == child).Name == "Nested parent/Child" &&
                overview.Folders.Single(folder => folder.Id == grand).Name == "Nested parent/Child/Leaf",
                "renaming a parent through the actual DLL does not cascade into child names");
            check(tree.Find(root) is { DisplayName: "Renamed parent", ItemCount: 0 } &&
                tree.Find(child) is { Parent: null, DisplayName: "Nested parent/Child", ItemCount: 1 } && tree.Find(grand)!.Parent!.Id == child,
                "after parent rename a child becomes a joined root and retains its own real descendants");
            overview = RustCore.DeleteFolders([root], token).Overview;
            check(overview.Folders.Any(folder => folder.Id == child) && overview.Folders.Any(folder => folder.Id == grand) &&
                overview.Items.Single(item => item.Id == login.Id).FolderId == child,
                "deleting a parent through the actual DLL preserves unselected descendants and their items");
            overview = RustCore.DeleteFolders([child], token).Overview;
            check(overview.Folders.Any(folder => folder.Id == grand) && overview.Items.Single(item => item.Id == login.Id).FolderId is null,
                "deleting a child retains its item in no-folder and does not delete an unselected grandchild");
            tree = FolderHierarchy.Build(overview.Folders, overview.Items);
            check(tree.Find(grand) is { Parent: null, DisplayName: "Nested parent/Child/Leaf" } && tree.Trail(grand).Length == 1,
                "deleted real ancestors leave no fabricated breadcrumb or navigation entry");
            overview = RustCore.SyncVault(token);
            check(overview.Items.Length == initial.Items.Length && overview.Folders.Any(folder => folder.Id == grand) &&
                overview.Items.Single(item => item.Id == login.Id).FolderId is null,
                "non-cascading deletion and retained items persist after a fresh sync");
        }
        finally
        {
            await Reset();
            RustCore.SyncVault(token);
        }
        async Task Reset()
        {
            using var response = await http.PostAsync("/control/reset", null);
            response.EnsureSuccessStatusCode();
        }
    }
}
