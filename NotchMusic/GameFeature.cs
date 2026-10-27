using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
using System.Text.RegularExpressions;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Interop;
using System.Windows.Media;
using System.Windows.Media.Imaging;

namespace NotchMusic;

// Halaman ketiga: daftar game pilihan sendiri (GameOrder), lalu kotak "+". Klik = buka.
// Game Steam masuk hanya lewat tombol "Scan"; semua game bisa dihapus dan dipindah.
public partial class MainWindow
{
    private const int GamePageIndex = 2;
    internal const int GameSlots = 8; // Termasuk kotak "+".
    private int _gameLoadVersion;

    private async void RefreshGames()
    {
        var version = ++_gameLoadVersion;
        var keys = _preferences.GameOrder.ToList();
        List<GameEntry> games;
        try
        {
            games = await Task.Run(() =>
            {
                var steamNames = keys.Any(SteamLibrary.IsAppId) ? SteamLibrary.RecentGames(int.MaxValue).ToDictionary(x => x.Key, x => x.Name) : [];
                // Game Steam yang sudah di-uninstall (manifest hilang) tidak ditampilkan. Bila Steam sama sekali
                // tidak terbaca, semua tetap tampil agar daftar tidak terhapus saat urutan disimpan.
                if (steamNames.Count > 0) keys = keys.Where(key => !SteamLibrary.IsAppId(key) || steamNames.ContainsKey(key)).ToList();
                return GameLibrary.WithIcons(keys.Select(key => SteamLibrary.IsAppId(key)
                    ? new GameEntry(key, steamNames.GetValueOrDefault(key, "Steam " + key), null, GameKind.Steam)
                    : GameLibrary.Custom(key)).ToList());
            });
        }
        catch (Exception ex) { Debug.WriteLine(ex); games = []; }
        if (_closed || version != _gameLoadVersion) return;
        // Game yang di-uninstall ikut keluar dari daftar tersimpan, supaya slotnya bisa dipakai lagi.
        if (games.Count > 0 && keys.Count < _preferences.GameOrder.Count) { _preferences.GameOrder = keys; SavePreferences(); }
        games.Add(GameEntry.AddButton);
        GameList.ItemsSource = games;
    }

    // Seret ikon ke posisi lain. Click tidak terpicu karena DoDragDrop mengambil alih mouse.
    private Point _gameDragStart;
    private GameEntry? _gameDragItem;

    private void Game_DragStart(object sender, MouseButtonEventArgs e)
    {
        _gameDragItem = (sender as FrameworkElement)?.Tag as GameEntry;
        _gameDragStart = e.GetPosition(this);
    }

    private void Game_DragMove(object sender, MouseEventArgs e)
    {
        if (_gameDragItem is not { Kind: not GameKind.Add } item || e.LeftButton != MouseButtonState.Pressed) return;
        var point = e.GetPosition(this);
        if (Math.Abs(point.X - _gameDragStart.X) < SystemParameters.MinimumHorizontalDragDistance
            && Math.Abs(point.Y - _gameDragStart.Y) < SystemParameters.MinimumVerticalDragDistance) return;
        _gameDragItem = null;
        // Selama seret, Windows memegang mouse dan WPF mengira mouse keluar notch: tahan penutupan.
        _gameDragging = true;
        var source = (UIElement)sender;
        source.Opacity = 0.4; // Ikon yang sedang dipindah.
        try { DragDrop.DoDragDrop(source, new DataObject(typeof(GameEntry), item), DragDropEffects.Move); }
        finally
        {
            source.Opacity = 1;
            HideDropMark();
            _gameDragging = false;
            if (!IsMouseOver) _collapseTimer.Start();
        }
    }

    private void Game_DragOver(object sender, DragEventArgs e)
    {
        e.Handled = true;
        var slot = DropSlot(sender, e);
        e.Effects = slot is null ? DragDropEffects.None : DragDropEffects.Move;
        if (slot is not { } target) { HideDropMark(); return; }
        // Pindah ke kanan = ditaruh sesudah ikon tujuan; ke kiri = sebelum. Kotak "+" = paling belakang (kirinya).
        ShowDropMark((Button)sender, target.From < target.To && target.Tile.Kind != GameKind.Add);
    }

    private void Game_DragLeave(object sender, DragEventArgs e) => HideDropMark();

    // Posisi asal/tujuan bila ikon dilepas di tile ini; null bila tidak mengubah urutan.
    private (int From, int To, GameEntry Tile)? DropSlot(object sender, DragEventArgs e)
    {
        if (e.Data.GetData(typeof(GameEntry)) is not GameEntry moved || GameList.ItemsSource is not List<GameEntry> games
            || (sender as FrameworkElement)?.Tag is not GameEntry tile) return null;
        var order = games.Where(x => x.Kind != GameKind.Add).ToList();
        var from = order.IndexOf(moved);
        var to = tile.Kind == GameKind.Add ? order.Count - 1 : order.IndexOf(tile);
        return from < 0 || to < 0 || from == to ? null : (from, to, tile);
    }

    private Button? _dropMarkTile;

    private void ShowDropMark(Button tile, bool right)
    {
        HideDropMark();
        if (tile.Template.FindName(right ? "DropRight" : "DropLeft", tile) is not FrameworkElement mark) return;
        mark.Visibility = Visibility.Visible;
        _dropMarkTile = tile;
    }

    private void HideDropMark()
    {
        if (_dropMarkTile is null) return;
        foreach (var name in new[] { "DropLeft", "DropRight" })
            if (_dropMarkTile.Template.FindName(name, _dropMarkTile) is FrameworkElement mark) mark.Visibility = Visibility.Collapsed;
        _dropMarkTile = null;
    }

    private void Game_Drop(object sender, DragEventArgs e)
    {
        e.Handled = true;
        HideDropMark();
        if (e.Data.GetData(typeof(GameEntry)) is not GameEntry moved || GameList.ItemsSource is not List<GameEntry> games) return;
        var order = games.Where(x => x.Kind != GameKind.Add).ToList();
        var from = order.IndexOf(moved);
        var target = (sender as FrameworkElement)?.Tag as GameEntry;
        var to = target is null || target.Kind == GameKind.Add ? order.Count - 1 : order.IndexOf(target);
        if (from < 0 || to < 0 || from == to) return;
        order.RemoveAt(from); order.Insert(to, moved);
        _preferences.GameOrder = order.Select(x => x.Key).ToList();
        SavePreferences();
        GameList.ItemsSource = order.Append(GameEntry.AddButton).ToList();
    }

    private void Game_Click(object sender, RoutedEventArgs e)
    {
        if ((sender as FrameworkElement)?.Tag is not GameEntry game) return;
        if (game.Kind == GameKind.Add) { AddCustomGame(); return; }
        GameStatus.Text = GameLibrary.Launch(game) ? $"Membuka {game.Name}…" : $"{game.Name} belum bisa dibuka.";
    }

    // Klik kanan game: menu kecil "Hapus dari daftar" (shortcut/game aslinya tidak disentuh).
    // Kotak "+" tidak punya menu; menu notch juga tidak ikut terbuka.
    private void Game_MenuOpening(object sender, ContextMenuEventArgs e)
    {
        if ((sender as FrameworkElement)?.Tag is not GameEntry { Kind: not GameKind.Add }) e.Handled = true;
    }

    private bool _gameMenuOpen;
    private void GameMenu_Opened(object sender, RoutedEventArgs e) { _gameMenuOpen = true; ControlsMenu_Opened(sender, e); }
    private void GameMenu_Closed(object sender, RoutedEventArgs e) { _gameMenuOpen = false; ControlsMenu_Closed(sender, e); }

    private void GameRemove_Click(object sender, RoutedEventArgs e)
    {
        if ((sender as FrameworkElement)?.DataContext is not GameEntry { Kind: not GameKind.Add } game) return;
        _preferences.GameOrder.RemoveAll(x => string.Equals(x, game.Key, StringComparison.OrdinalIgnoreCase));
        SavePreferences(); RefreshGames();
    }

    private void AddCustomGame()
    {
        var dialog = new Microsoft.Win32.OpenFileDialog
        {
            Title = "Pilih shortcut atau aplikasi game",
            Filter = "Shortcut atau aplikasi (*.lnk;*.url;*.exe)|*.lnk;*.url;*.exe",
            DereferenceLinks = false, // Simpan shortcut-nya sendiri, bukan file tujuannya.
            InitialDirectory = Environment.GetFolderPath(Environment.SpecialFolder.DesktopDirectory)
        };
        if (dialog.ShowDialog(this) != true) return;
        var path = dialog.FileName;
        if (_preferences.GameOrder.Contains(path, StringComparer.OrdinalIgnoreCase))
        { GameStatus.Text = "Game ini sudah ada di daftar."; return; }
        if (_preferences.GameOrder.Count >= GameSlots - 1)
        { GameStatus.Text = "Daftar penuh. Klik kanan game untuk menghapusnya."; return; }
        _preferences.GameOrder.Add(path); // Game baru di belakang; urutan yang sudah diatur tetap.
        SavePreferences(); RefreshGames();
        GameStatus.Text = $"{Path.GetFileNameWithoutExtension(path)} ditambahkan.";
    }

    // Isi slot kosong dengan game Steam yang terakhir dimainkan dan belum ada di daftar.
    private async void SteamScan_Click(object sender, RoutedEventArgs e)
    {
        var free = GameSlots - 1 - _preferences.GameOrder.Count;
        if (free <= 0) { GameStatus.Text = "Daftar penuh. Klik kanan game untuk menghapusnya."; return; }
        List<GameEntry> recent;
        try { recent = await Task.Run(() => SteamLibrary.RecentGames(int.MaxValue)); }
        catch (Exception ex) { Debug.WriteLine(ex); recent = []; }
        if (_closed) return;
        var added = recent.Select(x => x.Key).Except(_preferences.GameOrder).Take(GameSlots - 1 - _preferences.GameOrder.Count).ToList();
        if (added.Count == 0)
        { GameStatus.Text = recent.Count == 0 ? "Steam atau game Steam tidak ditemukan." : "Semua game Steam terbaru sudah ada."; return; }
        _preferences.GameOrder.AddRange(added);
        SavePreferences(); RefreshGames();
        GameStatus.Text = $"{added.Count} game Steam ditambahkan.";
    }
}

internal enum GameKind { Steam, Custom, Add }

// Key: appid untuk Steam, path shortcut untuk game tambahan.
internal sealed record GameEntry(string Key, string Name, ImageSource? Icon, GameKind Kind)
{
    public static readonly GameEntry AddButton = new("", "Tambah", null, GameKind.Add);
    public bool IsAdd => Kind == GameKind.Add;
    // Dipakai aksesibilitas Windows sebagai nama item. ToString bawaan record ikut mencetak Icon,
    // dan menyentuh gambar dari thread lain membuat aplikasi crash.
    public override string ToString() => Name;
}

internal static class GameLibrary
{
    // Ikon dimuat terpisah (WithIcons) agar hanya game yang tampil yang membaca file ikon.
    public static GameEntry Custom(string path) => new(path, Path.GetFileNameWithoutExtension(path), null, GameKind.Custom);

    public static List<GameEntry> WithIcons(List<GameEntry> games)
    {
        var steam = games.Any(x => x.Kind == GameKind.Steam) ? SteamLibrary.IconSource() : null;
        return games.Select(x => x with
        {
            Icon = x.Kind == GameKind.Custom ? (File.Exists(x.Key) ? ShellIcon(x.Key) : null)
                : steam is { } source ? SteamLibrary.Icon(source.Shortcuts, source.Cache, x.Key) : null
        }).ToList();
    }

    public static bool Launch(GameEntry game)
    {
        try
        {
            if (game.Kind == GameKind.Steam) return SteamLibrary.Launch(game.Key);
            if (game.Kind != GameKind.Custom || !File.Exists(game.Key)) return false;
            Process.Start(new ProcessStartInfo(game.Key) { UseShellExecute = true });
            return true;
        }
        catch (Exception ex) { Debug.WriteLine(ex); return false; }
    }

    // Ikon persis seperti di Explorer, termasuk shortcut aplikasi Store/Xbox yang tidak punya file tujuan.
    private static ImageSource? ShellIcon(string path)
    {
        IShellItemImageFactory? factory = null;
        var bitmap = IntPtr.Zero;
        try
        {
            var iid = typeof(IShellItemImageFactory).GUID;
            if (SHCreateItemFromParsingName(path, IntPtr.Zero, ref iid, out factory) != 0 || factory is null) return null;
            // 96 px: tajam untuk kotak 44 DIP sampai skala 200%. SIIGBF_BIGGERSIZEOK | SIIGBF_ICONONLY.
            if (factory.GetImage(new NativeSize { Width = 96, Height = 96 }, 0x1 | 0x4, out bitmap) != 0 || bitmap == IntPtr.Zero) return null;
            return FromDib(bitmap);
        }
        catch (Exception ex) { Debug.WriteLine(ex); return null; }
        finally
        {
            if (bitmap != IntPtr.Zero) DeleteObject(bitmap);
            if (factory is not null) Marshal.ReleaseComObject(factory);
        }
    }

    // HBITMAP 32-bit dari shell sudah premultiplied alpha. CreateBitmapSourceFromHBitmap membuang alpha
    // (latar jadi hitam), jadi piksel disalin langsung sebagai Pbgra32.
    private static BitmapSource? FromDib(IntPtr bitmap)
    {
        var info = new NativeBitmap();
        if (GetObject(bitmap, Marshal.SizeOf<NativeBitmap>(), ref info) == 0 || info.Bits == IntPtr.Zero
            || info.BitsPixel != 32 || info.Width <= 0 || info.Width > 512 || Math.Abs(info.Height) > 512) return null;
        var height = Math.Abs(info.Height);
        var stride = info.WidthBytes;
        var pixels = new byte[stride * height];
        Marshal.Copy(info.Bits, pixels, 0, pixels.Length);
        if (info.Height > 0) // DIB bottom-up: balik urutan baris.
        {
            var row = new byte[stride];
            for (var top = 0; top < height / 2; top++)
            {
                var bottom = height - 1 - top;
                Buffer.BlockCopy(pixels, top * stride, row, 0, stride);
                Buffer.BlockCopy(pixels, bottom * stride, pixels, top * stride, stride);
                Buffer.BlockCopy(row, 0, pixels, bottom * stride, stride);
            }
        }
        var image = BitmapSource.Create(info.Width, height, 96, 96, PixelFormats.Pbgra32, null, pixels, stride);
        image.Freeze();
        return image;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct NativeSize { public int Width, Height; }

    [StructLayout(LayoutKind.Sequential)]
    private struct NativeBitmap
    {
        public int Type, Width, Height, WidthBytes;
        public ushort Planes, BitsPixel;
        public IntPtr Bits;
    }

    [ComImport, Guid("bcc18b79-ba16-442f-80c4-8a59c30c463b"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    private interface IShellItemImageFactory
    {
        [PreserveSig] int GetImage(NativeSize size, int flags, out IntPtr bitmap);
    }

    [DllImport("shell32.dll", CharSet = CharSet.Unicode)]
    private static extern int SHCreateItemFromParsingName(string path, IntPtr bindContext, ref Guid iid,
        [MarshalAs(UnmanagedType.Interface)] out IShellItemImageFactory factory);
    [DllImport("gdi32.dll")]
    private static extern int GetObject(IntPtr handle, int size, ref NativeBitmap bitmap);
    [DllImport("gdi32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool DeleteObject(IntPtr handle);
}

internal static class SteamLibrary
{
    // Aplikasi Steam yang bukan game: Wallpaper Engine, Steamworks Redistributables, SteamVR.
    // shortcut: daftar tetap, tambah appid di sini bila ada aplikasi non-game lain yang ikut tampil.
    private static readonly HashSet<string> NotGames = ["431960", "228980", "250820"];

    public static bool IsAppId(string key) => Regex.IsMatch(key, "^[0-9]{1,10}$");

    private static string? SteamPath()
    {
        using var key = Microsoft.Win32.Registry.CurrentUser.OpenSubKey(@"Software\Valve\Steam");
        var path = key?.GetValue("SteamPath") as string;
        return string.IsNullOrWhiteSpace(path) || !Directory.Exists(path) ? null : path;
    }

    public static List<GameEntry> RecentGames(int count)
    {
        if (count <= 0) return [];
        var steam = SteamPath();
        if (steam is null) return [];
        // Folder library tambahan (mis. D:\Games\Steam) tercatat di libraryfolders.vdf.
        var libraries = new List<string> { steam };
        var folders = Path.Combine(steam, "steamapps", "libraryfolders.vdf");
        if (File.Exists(folders))
            foreach (Match match in Regex.Matches(File.ReadAllText(folders), "\"path\"\\s*\"([^\"]+)\""))
                libraries.Add(match.Groups[1].Value.Replace(@"\\", @"\"));
        var found = new List<(string Id, string Name, long LastPlayed)>();
        foreach (var library in libraries.Distinct(StringComparer.OrdinalIgnoreCase))
        {
            var apps = Path.Combine(library, "steamapps");
            if (!Directory.Exists(apps)) continue;
            foreach (var file in Directory.EnumerateFiles(apps, "appmanifest_*.acf"))
            {
                try
                {
                    if (new FileInfo(file).Length > 256 * 1024) continue;
                    var text = File.ReadAllText(file);
                    var id = Value(text, "appid"); var name = Value(text, "name");
                    // Belum pernah dimainkan (mis. Steamworks Redistributables) tidak ditampilkan.
                    if (id is null || name is null || NotGames.Contains(id)
                        || !long.TryParse(Value(text, "LastPlayed"), out var played) || played <= 0) continue;
                    found.Add((id, name, played));
                }
                catch (Exception ex) when (ex is IOException or UnauthorizedAccessException) { Debug.WriteLine(ex); }
            }
        }
        return found.DistinctBy(x => x.Id).OrderByDescending(x => x.LastPlayed).Take(count)
            .Select(x => new GameEntry(x.Id, x.Name, null, GameKind.Steam)).ToList();
    }

    public static (Dictionary<string, string> Shortcuts, string Cache)? IconSource()
        => SteamPath() is { } steam ? (ShortcutIcons(), Path.Combine(steam, "appcache", "librarycache")) : null;

    // appid hanya angka: aman dipakai di path dan URL steam://.
    private static string? Value(string text, string key)
    {
        var match = Regex.Match(text, $"\"{key}\"\\s*\"([^\"]*)\"", RegexOptions.IgnoreCase);
        if (!match.Success) return null;
        var value = match.Groups[1].Value.Trim();
        if (key == "appid" && !Regex.IsMatch(value, "^[0-9]{1,10}$")) return null;
        return value.Length == 0 ? null : value[..Math.Min(value.Length, 128)];
    }

    // Ikon shortcut Steam (sama dengan ikon di Desktop): .url berisi steam://rungameid/<id> dan IconFile=<.ico>.
    private static Dictionary<string, string> ShortcutIcons()
    {
        var icons = new Dictionary<string, string>();
        foreach (var folder in new[]
        {
            Environment.GetFolderPath(Environment.SpecialFolder.DesktopDirectory),
            Environment.GetFolderPath(Environment.SpecialFolder.CommonDesktopDirectory),
            Environment.GetFolderPath(Environment.SpecialFolder.Programs),
            Environment.GetFolderPath(Environment.SpecialFolder.CommonPrograms)
        })
        {
            if (string.IsNullOrEmpty(folder) || !Directory.Exists(folder)) continue;
            try
            {
                foreach (var file in Directory.EnumerateFiles(folder, "*.url", SearchOption.AllDirectories).Take(500))
                {
                    if (new FileInfo(file).Length > 4096) continue;
                    var text = File.ReadAllText(file);
                    var id = Regex.Match(text, @"steam://rungameid/([0-9]{1,10})", RegexOptions.IgnoreCase);
                    var icon = Regex.Match(text, @"^IconFile=(.+\.ico)\s*$", RegexOptions.IgnoreCase | RegexOptions.Multiline);
                    if (id.Success && icon.Success && File.Exists(icon.Groups[1].Value.Trim()))
                        icons.TryAdd(id.Groups[1].Value, icon.Groups[1].Value.Trim());
                }
            }
            catch (Exception ex) when (ex is IOException or UnauthorizedAccessException) { Debug.WriteLine(ex); }
        }
        return icons;
    }

    // Ikon .ico dari shortcut (resolusi tinggi), atau ikon 32x32 di cache Steam bila belum ada shortcut.
    public static ImageSource? Icon(Dictionary<string, string> shortcuts, string cache, string appId)
    {
        try
        {
            if (shortcuts.TryGetValue(appId, out var ico))
            {
                using var stream = File.OpenRead(ico);
                var frames = BitmapDecoder.Create(stream, BitmapCreateOptions.None, BitmapCacheOption.OnLoad).Frames;
                // Frame terkecil yang masih >= 64 px (tajam di skala 150%), atau yang terbesar.
                var frame = frames.Where(x => x.PixelWidth >= 64).OrderBy(x => x.PixelWidth).FirstOrDefault()
                    ?? frames.OrderByDescending(x => x.PixelWidth).First();
                // Salin piksel: frame .ico tetap terikat ke decoder di thread latar ini.
                var copy = new WriteableBitmap(frame);
                copy.Freeze();
                return copy;
            }
            var folder = Path.Combine(cache, appId);
            if (!Directory.Exists(folder)) return null;
            var jpg = Directory.EnumerateFiles(folder, "*.jpg").FirstOrDefault(x => Regex.IsMatch(Path.GetFileName(x), @"^[0-9a-f]{40}\.jpg$"));
            if (jpg is null) return null;
            var image = new BitmapImage();
            image.BeginInit();
            image.CacheOption = BitmapCacheOption.OnLoad; // File tidak dikunci.
            image.UriSource = new Uri(jpg);
            image.EndInit();
            image.Freeze();
            return image;
        }
        catch (Exception ex) { Debug.WriteLine(ex); return null; }
    }

    public static bool Launch(string appId)
    {
        if (!IsAppId(appId)) return false;
        try { Process.Start(new ProcessStartInfo("steam://rungameid/" + appId) { UseShellExecute = true }); return true; }
        catch (Exception ex) { Debug.WriteLine(ex); return false; }
    }
}
