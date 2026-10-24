using System.ComponentModel;
using System.Diagnostics;
using System.IO;
using System.Globalization;
using System.Runtime.InteropServices;
using System.Net;
using System.Net.Http;
using System.Text;
using System.Text.Json;
using System.Text.RegularExpressions;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;
using System.Windows.Input;
using System.Windows.Interop;
using System.Windows.Media;
using System.Windows.Media.Animation;
using System.Windows.Media.Imaging;
using System.Windows.Threading;
using Windows.Foundation;
using Windows.Media.Control;
using Windows.Storage.Streams;
using MediaManager = Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager;
using MediaSession = Windows.Media.Control.GlobalSystemMediaTransportControlsSession;
using Rect = System.Windows.Rect;
using Point = System.Windows.Point;

// Pencarian, konversi romaji, cache, dan file lirik. Dipindah dari MainWindow.xaml.cs tanpa perubahan isi.
namespace NotchMusic;

internal sealed class LrcRecord
{
    public long Id { get; set; }
    public string TrackName { get; set; } = "";
    public string ArtistName { get; set; } = "";
    public string AlbumName { get; set; } = "";
    public double Duration { get; set; }
    public bool Instrumental { get; set; }
    public string? PlainLyrics { get; set; }
    public string? SyncedLyrics { get; set; }
    [System.Text.Json.Serialization.JsonIgnore]
    public string Source { get; set; } = "LRCLIB";
    [System.Text.Json.Serialization.JsonIgnore]
    public bool IsRomanized { get; set; }
    [System.Text.Json.Serialization.JsonIgnore]
    public bool MachineRomanized { get; set; }
    [System.Text.Json.Serialization.JsonIgnore]
    public bool IsLocal { get; set; }
    [System.Text.Json.Serialization.JsonIgnore]
    public string? CacheFileName { get; set; }
    [System.Text.Json.Serialization.JsonIgnore]
    public string Language { get; set; } = "";
    [System.Text.Json.Serialization.JsonIgnore]
    public double? ImportedOffsetSeconds { get; set; }
    [System.Text.Json.Serialization.JsonIgnore]
    public LyricLine[]? ParsedLines { get; set; }
    [System.Text.Json.Serialization.JsonIgnore]
    public bool HasTiming => ParsedLines?.Length > 0 || !string.IsNullOrWhiteSpace(SyncedLyrics);
}

internal sealed class LrcServiceException(string message) : Exception(message) { }

internal static class LrcClient
{
    private static readonly SemaphoreSlim Gate = new(1, 1);
    private static readonly JsonSerializerOptions JsonOptions = new() { PropertyNameCaseInsensitive = true };
    private static DateTimeOffset _nextRequestUtc;
    private const int MaxRetries = 3;
    private static readonly TimeSpan MaxAutomaticWait = TimeSpan.FromSeconds(8);
    private const string RomajiMarker = @"\b(romaji|romanized|romanised|romanization|romanisation)\b";

    public static async Task<List<LrcRecord>> FindAsync(
        string title, string artist, string album, double duration, CancellationToken token,
        IProgress<string>? progress = null)
    {
        if (!string.IsNullOrWhiteSpace(artist))
        {
            var path = $"api/get?track_name={Uri.EscapeDataString(title)}&artist_name={Uri.EscapeDataString(artist)}";
            if (!string.IsNullOrWhiteSpace(album)) path += "&album_name=" + Uri.EscapeDataString(album);
            if (duration >= 1 && duration <= 3600)
                path += "&duration=" + duration.ToString("0.###", CultureInfo.InvariantCulture);
            using var json = await RequestAsync(path, token, progress);
            if (json?.RootElement.ValueKind == JsonValueKind.Object)
            {
                var exact = json.RootElement.Deserialize<LrcRecord>(JsonOptions);
                if (exact is not null && CompactRecord(exact)) return [exact];
            }
        }
        var matches = await SearchAsync(title, artist, token, progress);
        if (matches.Count == 0)
        {
            using var json = await RequestAsync("api/search?q=" + Uri.EscapeDataString(title), token, progress);
            matches = ParseRecords(json);
        }
        return matches;
    }

    private static async Task<List<LrcRecord>> SearchAsync(
        string title, string artist, CancellationToken token, IProgress<string>? progress)
    {
        var path = "api/search?track_name=" + Uri.EscapeDataString(title);
        if (!string.IsNullOrWhiteSpace(artist)) path += "&artist_name=" + Uri.EscapeDataString(artist);
        using var json = await RequestAsync(path, token, progress);
        return ParseRecords(json);
    }

    public static async Task<List<LrcRecord>> FindRomajiAsync(
        string title, string artist, List<LrcRecord> originals, CancellationToken token,
        IProgress<string>? progress = null)
    {
        var candidates = originals.Where(IsRomaji).ToList();
        if (candidates.Count == 0)
            candidates = (await SearchAsync(title, artist, token, progress)).Where(IsRomaji).ToList();
        if (candidates.Count == 0)
        {
            var query = Uri.EscapeDataString($"{title} {artist} romaji");
            using var json = await RequestAsync("api/search?q=" + query, token, progress);
            candidates = ParseRecords(json).Where(IsRomaji).ToList();
        }
        return candidates.DistinctBy(r => r.Id).Take(20).ToList();
    }

    private static List<LrcRecord> ParseRecords(JsonDocument? document)
    {
        if (document?.RootElement.ValueKind != JsonValueKind.Array) return [];
        var records = new List<LrcRecord>(20);
        // Jangan membuat seluruh daftar objek lalu membuang mayoritas hasilnya.
        foreach (var item in document.RootElement.EnumerateArray().Take(20))
        {
            if (item.ValueKind != JsonValueKind.Object) continue;
            var record = item.Deserialize<LrcRecord>(JsonOptions);
            if (record is not null && CompactRecord(record)) records.Add(record);
        }
        return records;
    }

    private static bool CompactRecord(LrcRecord record)
    {
        if (record.SyncedLyrics?.Length > LyricLimits.SourceCharacters
            || record.PlainLyrics?.Length > LyricLimits.SourceCharacters) return false;
        record.TrackName = (record.TrackName ?? "")[..Math.Min(record.TrackName?.Length ?? 0, 512)];
        record.ArtistName = (record.ArtistName ?? "")[..Math.Min(record.ArtistName?.Length ?? 0, 512)];
        record.AlbumName = (record.AlbumName ?? "")[..Math.Min(record.AlbumName?.Length ?? 0, 512)];
        // Tolak sumber berlebihan; jangan menampilkan potongan lirik seolah-olah lengkap.
        // LRC sudah memuat teks, jadi salinan plain tidak perlu ditahan.
        if (!string.IsNullOrWhiteSpace(record.SyncedLyrics)
            && Regex.IsMatch(record.SyncedLyrics, @"\[\d{1,3}:[0-5]\d(?:[.:]\d{1,3})?\]"))
            record.PlainLyrics = null;
        return true;
    }

    public static bool HasKana(string text)
        => Regex.IsMatch(text, @"[\u3041-\u3096\u30A1-\u30FA\uFF66-\uFF9D]");

    public static bool IsMarkedRomaji(LrcRecord record)
        => record.IsRomanized || Regex.IsMatch(record.TrackName + " " + record.AlbumName, RomajiMarker, RegexOptions.IgnoreCase);

    private static bool IsRomaji(LrcRecord record)
    {
        var text = record.PlainLyrics ?? record.SyncedLyrics ?? "";
        // Tidak perlu membentuk salinan teks tanpa timestamp hanya untuk memeriksa aksara.
        // Teks Latin saja bisa juga terjemahan Inggris; perlu label romaji eksplisit.
        return IsMarkedRomaji(record) && Regex.IsMatch(text, "[A-Za-z]")
            && !Regex.IsMatch(record.TrackName + " " + record.AlbumName,
                @"\b(translation|translated|terjemahan)\b", RegexOptions.IgnoreCase)
            && !Regex.IsMatch(text, @"[\u3040-\u30FF\u3400-\u9FFF\uFF66-\uFF9D]");
    }

    public static string GetText(LrcRecord record)
    {
        if (!string.IsNullOrWhiteSpace(record.PlainLyrics)) return record.PlainLyrics.Trim();
        var text = record.SyncedLyrics ?? "";
        text = Regex.Replace(text, @"(?m)^\s*(?:\[\d+:\d+(?:[.:]\d+)?\])+\s*", "");
        text = Regex.Replace(text, @"<\d{1,3}:\d{2}(?:[.:]\d{1,3})?>", "");
        return Regex.Replace(text, @"(?m)^\[(?:ar|al|ti|by|offset):[^\]]*\]\s*$", "").Trim();
    }

    private static string Normalize(string text)
    {
        text = Regex.Replace(text ?? "", RomajiMarker, "", RegexOptions.IgnoreCase);
        return new string(text.Normalize(NormalizationForm.FormKC)
            .Where(char.IsLetterOrDigit).Select(char.ToLowerInvariant).ToArray());
    }

    public static bool SameSong(LrcRecord record, string title, string artist, double duration)
        => !string.IsNullOrWhiteSpace(artist)
           && Normalize(record.TrackName) == Normalize(title)
           && Normalize(record.ArtistName) == Normalize(artist)
           && (duration <= 0 || Math.Abs(record.Duration - duration) <= 3);

    private static async Task<JsonDocument?> RequestAsync(
        string path, CancellationToken token, IProgress<string>? progress)
    {
        await Gate.WaitAsync(token);
        try
        {
            // Satu permintaan awal + paling banyak tiga percobaan ulang.
            // Tidak ada polling setelah batas ini atau setelah jendela ditutup.
            for (var attempt = 0; attempt <= MaxRetries; attempt++)
            {
                token.ThrowIfCancellationRequested();
                var wait = _nextRequestUtc - DateTimeOffset.UtcNow;
                if (wait > MaxAutomaticWait)
                    throw new LrcServiceException($"Layanan masih meminta jeda. Tunggu minimal {Math.Ceiling(wait.TotalSeconds)} detik sebelum Cari ulang, atau pilih Cari di browser.");
                if (wait > TimeSpan.Zero)
                {
                    if (attempt == 0 && wait.TotalSeconds > 0.5)
                        progress?.Report($"Menunggu jeda layanan {Math.Ceiling(wait.TotalSeconds)} detik…");
                    await Task.Delay(wait, token);
                }

                progress?.Report(attempt == 0 ? "Mencari lirik di LRCLIB…"
                    : $"Mencoba kembali ({attempt}/{MaxRetries})…");
                using var timeout = CancellationTokenSource.CreateLinkedTokenSource(token);
                timeout.CancelAfter(TimeSpan.FromSeconds(15));
                using var response = await LyricHttp.Client.GetAsync("https://lrclib.net/" + path,
                    HttpCompletionOption.ResponseHeadersRead, timeout.Token);
                if (response.StatusCode == HttpStatusCode.TooManyRequests ||
                    response.StatusCode == HttpStatusCode.ServiceUnavailable)
                {
                    var rateLimited = response.StatusCode == HttpStatusCode.TooManyRequests;
                    var retry = response.Headers.RetryAfter;
                    var backoff = TimeSpan.FromSeconds(1 << attempt);
                    var delay = retry?.Delta ?? (retry?.Date is DateTimeOffset date
                        ? date - DateTimeOffset.UtcNow
                        : rateLimited ? TimeSpan.FromSeconds(30) : backoff);
                    // Hormati Retry-After, dengan jeda minimal yang meningkat: 1, 2, 4 detik.
                    // Jeda panjang tidak dipotong agar tidak melanggar batas layanan.
                    if (delay < backoff) delay = backoff;
                    try { _nextRequestUtc = DateTimeOffset.UtcNow + delay; }
                    catch (ArgumentOutOfRangeException) { _nextRequestUtc = DateTimeOffset.MaxValue; }
                    var reason = rateLimited ? "Pencarian sementara dibatasi layanan."
                        : "Layanan lirik sedang sibuk.";
                    if (attempt == MaxRetries || delay > MaxAutomaticWait)
                    {
                        var exhausted = attempt == MaxRetries ? $"Sudah dicoba ulang {MaxRetries} kali. " : "";
                        throw new LrcServiceException($"{reason} {exhausted}Tunggu minimal {Math.Ceiling(delay.TotalSeconds)} detik sebelum Cari ulang, atau pilih Cari di browser.");
                    }
                    progress?.Report($"{reason} Mencoba lagi otomatis dalam {Math.Ceiling(delay.TotalSeconds)} detik ({attempt + 1}/{MaxRetries})…");
                    continue; // Respons dilepas sebelum jeda percobaan berikutnya.
                }
                if (response.StatusCode == HttpStatusCode.NotFound) return null;
                response.EnsureSuccessStatusCode();
                return await LyricHttp.ReadJsonAsync(response, timeout.Token);
            }
            throw new LrcServiceException("Lirik belum bisa dimuat. Coba Cari di browser.");
        }
        finally
        {
            var earliest = DateTimeOffset.UtcNow.AddMilliseconds(300);
            if (_nextRequestUtc < earliest) _nextRequestUtc = earliest;
            Gate.Release();
        }
    }
}

// Semua hasil dan teks dibatasi; tidak ada model AI, WebView, atau unduhan katalog lagu.
internal enum LyricsMode { Auto, Original, Romaji }
internal static class LyricsLanguage
{
    public static bool PrefersRomaji(LyricsMode mode, LrcRecord record)
        => mode != LyricsMode.Original && !record.Instrumental
            && (LrcClient.IsMarkedRomaji(record) || UnisonClient.IsJapanese(record));
}
internal sealed record LyricLine(double Seconds, string Text);
internal sealed record LyricCandidate(LrcRecord Record, double Score, bool Confident)
{
    public string DisplayName => Record.CacheFileName is not null
        ? $"Tersimpan · {Record.TrackName} — {Record.ArtistName}"
            + (Record.Duration > 0 ? $" · {(int)(Record.Duration / 60)}:{(int)Record.Duration % 60:00}" : "")
            + (Record.IsRomanized ? " · romaji" : "")
        : Record.Source == "Better Lyrics"
        ? $"Better Lyrics · romaji berwaktu · {Record.TrackName} — {Record.ArtistName}" + (Confident ? "" : " · periksa hasil")
        : (Record.IsLocal ? "Lokal · " : $"{Record.Source} · {Score:P0} mirip · ")
            + $"{Record.TrackName} — {Record.ArtistName} · " + (Record.HasTiming ? "berwaktu" : "teks biasa")
            + (Record.MachineRomanized ? " · romaji hasil konversi" : "");
}
internal sealed record LyricsSearchResult(List<LyricCandidate> Choices,
    bool WantRomaji, string Note);

internal sealed class LyricDocument
{
    public LyricLine[] Lines { get; private init; } = [];
    private string _plainText = "";
    // Hanya jendela Aa yang membutuhkan satu string berisi seluruh lagu.
    public string PlainText => IsSynced ? string.Join(Environment.NewLine, Lines.Select(x => x.Text)) : _plainText;
    public bool IsSynced => Lines.Length > 0;
    public bool Instrumental { get; private init; }
    public string[] Preview { get; private init; } = [];
    private static readonly Regex Stamp = new(@"\[(?<m>\d{1,4}):(?<s>\d{2})(?:[.:](?<f>\d{1,3}))?\]", RegexOptions.CultureInvariant);

    public static LyricDocument From(LrcRecord record)
    {
        if (record.ParsedLines is { Length: > 0 } parsed) return Synced(parsed, record.Instrumental);
        var text = record.SyncedLyrics ?? "";
        if (text.Length > LyricLimits.TextFileCharacters) return new();
        var offsetMatch = Regex.Match(text, @"\[offset:([+-]?\d{1,8})\]", RegexOptions.IgnoreCase);
        var offset = offsetMatch.Success && double.TryParse(offsetMatch.Groups[1].Value,
            NumberStyles.Integer, CultureInfo.InvariantCulture, out var ms) ? ms / 1000 : 0;
        var lines = new List<LyricLine>();
        using var reader = new StringReader(text);
        var rawCount = 0;
        var parsedCharacters = 0;
        while (reader.ReadLine() is string raw)
        {
            if (++rawCount > LyricLimits.Lines + 32) return new();
            var stamps = Stamp.Matches(raw);
            if (stamps.Count == 0) continue;
            if (stamps.Count > 8) return new();
            var words = Stamp.Replace(raw, "").Trim();
            words = Regex.Replace(words, @"<\d{1,3}:\d{2}(?:[.:]\d{1,3})?>", "");
            if (words.Length > 4096) return new();
            foreach (Match stamp in stamps)
            {
                var seconds = int.Parse(stamp.Groups["s"].Value, CultureInfo.InvariantCulture);
                if (seconds >= 60) continue;
                var fraction = stamp.Groups["f"].Value;
                var time = int.Parse(stamp.Groups["m"].Value, CultureInfo.InvariantCulture) * 60 + seconds;
                var part = fraction.Length == 0 ? 0 : int.Parse(fraction, CultureInfo.InvariantCulture) / Math.Pow(10, fraction.Length);
                if (time + part - offset > 86400) return new();
                parsedCharacters += words.Length + 3;
                if (parsedCharacters > LyricLimits.SourceCharacters) return new();
                lines.Add(new LyricLine(Math.Max(0, time + part - offset), words));
                if (lines.Count > LyricLimits.Lines) return new();
            }
        }
        if (lines.Count > 0)
        {
            var sorted = lines.GroupBy(x => x.Seconds).OrderBy(g => g.Key)
                .Select(g => new LyricLine(g.Key, string.Join(" / ", g.Select(x => x.Text).Distinct()))).ToArray();
            if (sorted.Any(x => x.Text.Length > 4096)) return new();
            record.ParsedLines = sorted;
            record.SyncedLyrics = null; record.PlainLyrics = null;
            return Synced(sorted, record.Instrumental);
        }
        var plain = record.Instrumental ? "Instrumental — tidak ada lirik." : LrcClient.GetText(record);
        if (plain.Length > LyricLimits.SourceCharacters) return new();
        var preview = new List<string>(2);
        using var plainReader = new StringReader(plain);
        while (preview.Count < 2 && plainReader.ReadLine() is string row)
            if (!string.IsNullOrWhiteSpace(row)) preview.Add(row.Trim());
        return new LyricDocument { _plainText = plain, Instrumental = record.Instrumental, Preview = preview.ToArray() };
    }

    private static LyricDocument Synced(LyricLine[] lines, bool instrumental)
        => new() { Lines = lines, Instrumental = instrumental,
            Preview = lines.Where(x => !string.IsNullOrWhiteSpace(x.Text)).Take(2).Select(x => x.Text).ToArray() };

    public int IndexAt(double seconds)
    {
        var low = 0;
        var high = Lines.Length - 1;
        while (low <= high)
        {
            var mid = low + (high - low) / 2;
            if (Lines[mid].Seconds <= seconds) low = mid + 1;
            else high = mid - 1;
        }
        return high;
    }
}

internal static class SongMatcher
{
    private const string Roman = @"\b(?:romaji|romanized|romanised|romanization|romanisation)\b";
    private const string Video = @"\b(?:official\s+(?:music\s+)?(?:video|audio|lyric\s+video|mv)|music\s+video|lyric\s+video|lyrics\s+video|visuali[sz]er|official\s+mv|hd|4k)\b";
    public static bool IsRomajiLabel(string text) => Regex.IsMatch(text, Roman, RegexOptions.IgnoreCase);
    public static bool IsTranslation(string text) => Regex.IsMatch(text,
        @"\b(?:english|indonesian|spanish|french|german|portuguese|russian|korean|japanese|chinese)\s+translation\b|\bterjemahan\b|Genius\s+(?!Romanizations)\w*\s*Translations", RegexOptions.IgnoreCase);
    private static string Limit(string value, int max = 192) => value.Length > max ? value[..max] : value;
    public static string CleanTitle(string text)
    {
        text = Limit(text ?? "", 512).Replace('\u200B', ' ');
        text = Regex.Replace(text, Video, "", RegexOptions.IgnoreCase);
        text = Regex.Replace(text, Roman, "", RegexOptions.IgnoreCase);
        text = Regex.Replace(text, @"(?:\s+|[\(\[【])(?:M/?V|PV)(?:[\)\]】])?\s*$", "", RegexOptions.IgnoreCase);
        text = Regex.Replace(text, @"[\(\[【]\s*[\)\]】]", "");
        return Regex.Replace(text, @"\s+", " ").Trim(' ', '-', '–', '|');
    }
    public static string CleanArtist(string text) => Regex.Replace(Limit(text ?? "", 256),
        @"\s*(?:-\s*Topic|Official(?:\s+Channel)?)\s*$", "", RegexOptions.IgnoreCase).Trim();
    public static (string Title, string Artist) CleanQuery(string title, string artist)
    {
        title = CleanTitle(title);
        artist = CleanArtist(artist);
        // Format YouTube seperti Artis『Judul』Official Music Video.
        var framed = Regex.Match(title, @"^(?<artist>[^『「]{1,128})[『「](?<title>[^』」]{1,512})[』」]\s*$");
        if (framed.Success)
        {
            var lead = CleanArtist(framed.Groups["artist"].Value);
            var ageSuffix = artist.StartsWith(lead, StringComparison.OrdinalIgnoreCase)
                && Regex.IsMatch(artist[lead.Length..], @"^\s*\(\d{1,2}\)\s*$");
            if (Similarity(lead, artist) >= 0.95 || ageSuffix || artist.Length == 0)
            { artist = lead; title = framed.Groups["title"].Value.Trim(); }
        }
        var split = Regex.Match(title, @"^(?<left>.+?)\s+[-–—]\s+(?<right>.+)$");
        if (split.Success)
        {
            var left = split.Groups["left"].Value.Trim();
            var right = split.Groups["right"].Value.Trim();
            if (Similarity(CleanArtist(left), artist) >= 0.95)
                title = right;
            else if (Similarity(CleanArtist(right), artist) >= 0.95)
                title = left; // Contoh: Heart Forecast - Eve MV, artis Eve.
            else if (string.IsNullOrWhiteSpace(artist))
            { artist = CleanArtist(left); title = right; }
        }
        return (title, artist);
    }
    private static string Normalize(string text)
    {
        text = Limit(text).Normalize(NormalizationForm.FormKC).Normalize(NormalizationForm.FormD);
        var chars = text.Where(c => CharUnicodeInfo.GetUnicodeCategory(c) != UnicodeCategory.NonSpacingMark)
            .Select(c => char.IsLetterOrDigit(c) ? char.ToLowerInvariant(c) : ' ').ToArray();
        return Limit(Regex.Replace(new string(chars), @"\s+", " ").Trim());
    }
    public static double Similarity(string first, string second)
    {
        var a = Normalize(first);
        var b = Normalize(second);
        if (a.Length == 0 || b.Length == 0) return 0;
        if (a == b) return 1;
        if (a.Length < b.Length) (a, b) = (b, a);
        // One bounded row, at most 193 integers. No quadratic matrix allocation.
        Span<int> row = stackalloc int[b.Length + 1];
        for (var j = 0; j <= b.Length; j++) row[j] = j;
        for (var i = 1; i <= a.Length; i++)
        {
            var diagonal = row[0]; row[0] = i;
            for (var j = 1; j <= b.Length; j++)
            {
                var old = row[j];
                row[j] = Math.Min(Math.Min(row[j] + 1, row[j - 1] + 1), diagonal + (a[i - 1] == b[j - 1] ? 0 : 1));
                diagonal = old;
            }
        }
        var edit = 1.0 - (double)row[b.Length] / a.Length;
        var left = a.Split(' ').ToHashSet();
        var right = b.Split(' ').ToHashSet();
        var dice = 2.0 * left.Intersect(right).Count() / (left.Count + right.Count);
        return Math.Max(edit, dice * 0.96);
    }
    public static string[] Aliases(string title, string artist)
    {
        title = CleanQuery(title, artist).Title;
        var aliases = new List<string> { title };
        foreach (Match match in Regex.Matches(title, @"[\(（]([^\)）]+)[\)）]").Cast<Match>().Take(4))
        {
            var alias = match.Groups[1].Value.Trim();
            if (Regex.IsMatch(title, @"[\u3040-\u30FF\u3400-\u9FFF]") &&
                !Regex.IsMatch(alias, @"\b(?:live|remix|cover|version|ver|acoustic|instrumental|feat|ft)\b", RegexOptions.IgnoreCase))
                aliases.Add(alias);
        }
        var noAlias = Regex.Replace(title, @"[\(（][^\)）]+[\)）]", "").Trim();
        // Version labels are meaningful and must remain part of matching.
        if (!Regex.IsMatch(title, @"\b(?:live|remix|cover|acoustic|instrumental|version|ver)\b", RegexOptions.IgnoreCase)) aliases.Add(noAlias);
        return aliases.Where(x => !string.IsNullOrWhiteSpace(x)).Distinct().Take(6).ToArray();
    }
    public static double TitleScore(string title, string artist, string query)
        => Aliases(title, artist).Select(x => Similarity(x, query)).DefaultIfEmpty(0).Max();
    public static List<LyricCandidate> Rank(IEnumerable<LrcRecord> records, string title, string artist, double duration,
        IReadOnlyList<string>? queryAliases = null)
    {
        return records.Take(20).Select(r =>
        {
            var ts = (queryAliases ?? new[] { title }).Take(6).Select(q => TitleScore(r.TrackName, r.ArtistName, q)).DefaultIfEmpty(0).Max();
            var ars = Similarity(CleanArtist(r.ArtistName), artist);
            var score = string.IsNullOrWhiteSpace(artist) ? ts * 0.85 : 0.75 * ts + 0.25 * ars;
            var durationFits = duration <= 0 || (r.Duration > 0 && Math.Abs(r.Duration - duration) <= 3);
            if (!durationFits) score *= 0.8;
            return new LyricCandidate(r, score, ts >= 0.90 && ars >= 0.82 && score >= 0.90 && durationFits);
        }).Where(c => c.Score >= 0.40 || (LrcClient.IsMarkedRomaji(c.Record) && Similarity(c.Record.ArtistName, artist) >= 0.85))
            .OrderByDescending(c => c.Score).ThenByDescending(c => c.Record.HasTiming).ToList();
    }
    public static LyricCandidate? AutoChoice(List<LyricCandidate> choices)
    {
        var first = choices.FirstOrDefault();
        if (first is null || !first.Confident) return null;
        var next = choices.Skip(1).FirstOrDefault(x => Normalize(x.Record.TrackName) != Normalize(first.Record.TrackName)
            || Normalize(x.Record.ArtistName) != Normalize(first.Record.ArtistName)
            || Math.Abs(x.Record.Duration - first.Record.Duration) > 3);
        return next is null || first.Score - next.Score >= 0.06 ? first : null;
    }
}

internal static class LyricsSearch
{
    public static async Task<LyricsSearchResult> FindAsync(string title, string artist, string album, double duration,
        LyricsMode mode, CancellationToken token, IProgress<string>? progress)
    {
        var rawTitle = title; var rawArtist = artist; var japaneseHint = false;
        (title, artist) = SongMatcher.CleanQuery(title, artist);
        album = album[..Math.Min(album.Length, 512)];
        var wantRomaji = false;
        var records = new List<LrcRecord>();
        var notes = new List<string>();
        var betterQueries = new HashSet<string>(StringComparer.Ordinal);
        var unisonQueries = new HashSet<string>(StringComparer.Ordinal);
        void AddNote(Exception ex, string provider)
            => notes.Add(ex is LrcServiceException ? ex.Message : provider + " belum dapat dihubungi.");
        string Notes() => string.Join(" ", notes.Distinct().Take(5));
        List<LyricCandidate> Rank(IEnumerable<LrcRecord> input) => SongMatcher.Rank(input
            .Where(x => !SongMatcher.IsTranslation(x.TrackName + " " + x.AlbumName))
            .DistinctBy(x => (x.Source, x.Id))
            .OrderByDescending(x => LrcClient.IsMarkedRomaji(x) && x.HasTiming), title, artist, duration);
        void DetectJapanese()
        {
            var first = SongMatcher.AutoChoice(Rank(records));
            wantRomaji = first is not null ? LyricsLanguage.PrefersRomaji(mode, first.Record) : japaneseHint;
        }
        LyricsSearchResult Result()
        {
            DetectJapanese();
            // Romaji adalah pilihan penyajian Jepang, bukan filter yang membuang bahasa lain.
            var eligible = mode == LyricsMode.Original ? records.Where(x => !LrcClient.IsMarkedRomaji(x)) : records;
            var ranked = Rank(eligible);
            var automatic = SongMatcher.AutoChoice(ranked);
            var retained = LyricLimits.Retain(ranked);
            if (automatic is null && retained.Count > 0) retained[0] = retained[0] with { Confident = false };
            return new(retained, wantRomaji, Notes());
        }
        bool Ready(LyricsSearchResult result)
        {
            var selected = SongMatcher.AutoChoice(result.Choices);
            return selected is not null && (selected.Record.Instrumental || selected.Record.HasTiming);
        }
        async Task<LyricCandidate?> TryBetter(string t, string a, string al, double d)
        {
            var key = $"{t}\u001f{a}\u001f{al}\u001f{Math.Round(d)}";
            if (betterQueries.Count >= 2 || !betterQueries.Add(key)) return null;
            token.ThrowIfCancellationRequested();
            progress?.Report("Mencari romaji berwaktu di Better Lyrics…");
            var result = await BetterLyricsClient.FindRomajiAsync(t, a, al, d, token);
            if (result.Candidate is null) notes.Add(result.Note);
            return result.Candidate;
        }
        async Task TryUnison(string t, string a, string al, double d)
        {
            var key = $"{t}\u001f{a}\u001f{al}\u001f{Math.Round(d)}";
            if (unisonQueries.Count >= 2 || !unisonQueries.Add(key)) return;
            progress?.Report("Mencari teks dan penanda waktu di Unison…");
            using var budget = CancellationTokenSource.CreateLinkedTokenSource(token);
            budget.CancelAfter(TimeSpan.FromSeconds(12));
            try
            {
                var extra = await UnisonClient.FindAsync(t, a, al, d, mode != LyricsMode.Original, budget.Token);
                records = extra.Concat(Rank(records).Select(x => x.Record))
                    .DistinctBy(x => (x.Source, x.Id)).Take(20).ToList();
            }
            catch (OperationCanceledException) when (token.IsCancellationRequested) { throw; }
            catch (Exception ex) { AddNote(ex, "Unison"); }
        }
        LyricsSearchResult FromBetter(LyricCandidate candidate)
            => new([candidate], true, "Romaji berwaktu dari Better Lyrics. " + Notes());

        // Video YouTube tidak punya album dan judulnya sering berbeda dari judul rilis.
        // Kenali lagu aslinya dulu agar LRCLIB menerima judul, artis, dan durasi audio yang benar.
        if (album.Length == 0)
        {
            using var budget = CancellationTokenSource.CreateLinkedTokenSource(token);
            budget.CancelAfter(TimeSpan.FromSeconds(8));
            try
            {
                progress?.Report("Mengenali lagu asli dari judul video…");
                var song = await SongIdentifier.FindAsync(rawTitle, rawArtist, duration, budget.Token);
                if (song is not null)
                {
                    // Album katalog bisa berbeda dari LRCLIB; judul, artis, dan durasi cukup.
                    title = song.Title; artist = song.Artist;
                    if (song.Duration > 0) duration = song.Duration;
                }
            }
            catch (OperationCanceledException) when (token.IsCancellationRequested) { throw; }
            catch (Exception ex) { AddNote(ex, "Katalog lagu"); }
        }
        japaneseHint = mode == LyricsMode.Romaji || (mode == LyricsMode.Auto && LrcClient.HasKana(title));

        progress?.Report("Mencari teks dan penanda waktu di LRCLIB…");
        using (var budget = CancellationTokenSource.CreateLinkedTokenSource(token))
        {
            budget.CancelAfter(TimeSpan.FromSeconds(16));
            try { records.AddRange(await LrcClient.FindAsync(title, artist, album, duration, budget.Token, progress)); }
            catch (OperationCanceledException) when (token.IsCancellationRequested) { throw; }
            catch (Exception ex) { AddNote(ex, "LRCLIB"); }
        }
        // LRCLIB dan Unison mencari lirik semua bahasa terlebih dahulu, termasuk teks tanpa waktu.
        if (!Ready(Result())) await TryUnison(title, artist, album, duration);
        DetectJapanese();
        var original = SongMatcher.AutoChoice(Rank(records));
        if (wantRomaji && original?.Record.Instrumental != true
            && (original is null || !LrcClient.IsMarkedRomaji(original.Record)))
        {
            var canonical = original?.Record;
            var better = await TryBetter(canonical?.TrackName ?? title, canonical?.ArtistName ?? artist,
                string.IsNullOrWhiteSpace(canonical?.AlbumName) ? album : canonical!.AlbumName,
                canonical?.Duration > 0 ? canonical.Duration : duration);
            if (better is { Confident: true } || (better is not null && records.Count == 0)) return FromBetter(better!);
            if (better is not null) notes.Add("Kecocokan romaji belum kuat; lirik asli tetap menjadi pilihan.");
            await TryUnison(canonical?.TrackName ?? title, canonical?.ArtistName ?? artist,
                string.IsNullOrWhiteSpace(canonical?.AlbumName) ? album : canonical!.AlbumName,
                canonical?.Duration > 0 ? canonical.Duration : duration);
            var found = Result();
            if (Ready(found)) return found; // Baris Jepang yang cocok dikonversi setelah dipilih.
            using var budget = CancellationTokenSource.CreateLinkedTokenSource(token);
            budget.CancelAfter(TimeSpan.FromSeconds(8));
            try { records.AddRange(await LrcClient.FindRomajiAsync(title, artist, records, budget.Token, progress)); }
            catch (OperationCanceledException) when (token.IsCancellationRequested) { throw; }
            catch (Exception ex) { AddNote(ex, "Romaji LRCLIB"); }
        }
        token.ThrowIfCancellationRequested();
        return Result();
    }
}

internal sealed record IdentifiedSong(string Title, string Artist, string Album, double Duration);

// Mencocokkan judul video bebas dengan katalog Deezer (publik, tanpa token).
// Hasil hanya dipakai bila judul dan artis/durasi sama-sama cocok; selain itu pencarian lama berjalan.
internal static class SongIdentifier
{
    private const RegexOptions Flags = RegexOptions.IgnoreCase | RegexOptions.CultureInvariant;
    private const string LatinNoise = @"\b(?:official|music\s*video|video|audio|lyrics?|lirik|mv|pv|4k|hd|\d{3,4}p|sub(?:title)?s?|terjemahan|romaji|eng|english|indo|full\s*ver(?:sion)?|ver\.?|version|cover|live|remaster(?:ed)?|visuali[sz]er|op|ed|opening|ending|ost|anime|nightcore|slowed|reverb|sped\s*up|8d|karaoke|tiktok|color\s*coded)\b";
    private const string CjkNoise = "歌詞|字幕|歌ってみた|弾いてみた|叩いてみた|公式|主題歌|オープニング|エンディング|カラオケ|フル";
    private const string Separator = @"\s[-–—~]\s|[|｜／]|\s/\s";
    private const string Version = @"(?:live|remix|acoustic|instrumental|karaoke|sped\s*up|slowed|remaster(?:ed)?|tour|unplugged)";

    static SongIdentifier() => SelfCheck();

    private static string Key(string text) => new((text ?? "").Normalize(NormalizationForm.FormKC)
        .Where(char.IsLetterOrDigit).Select(char.ToLowerInvariant).ToArray());

    private static string Artist(string artist)
        => Regex.Replace(SongMatcher.CleanArtist(artist), @"\s*VEVO$", "", Flags).Trim();

    // Judul bersih (pemisah tetap ada), bagian dalam 『』/「」, dan potongan per pemisah.
    public static (string Whole, string Quoted, string[] Parts) Clean(string rawTitle)
    {
        var text = (rawTitle ?? "")[..Math.Min(rawTitle?.Length ?? 0, 512)];
        var quoted = Regex.Match(text, @"[『「]([^』」]{1,128})[』」]").Groups[1].Value.Trim();
        text = Regex.Replace(text, @"#\S+", " ");
        text = Regex.Replace(text, @"[\(\[【（［〔]([^\)\]】）］〕]{0,128})[\)\]】）］〕]", m =>
            Regex.IsMatch(m.Groups[1].Value, LatinNoise, Flags) || Regex.IsMatch(m.Groups[1].Value, CjkNoise) ? " " : m.Value);
        text = Regex.Replace(text, @"\s(?:feat\.?|ft\.?|prod\.?\s*by|prod\.)\s[^\(\[【|｜／-]+", " ", Flags);
        text = Regex.Replace(SongMatcher.CleanTitle(text), @"[『』「」""]", " ");
        text = Regex.Replace(text, @"\s+", " ").Trim(' ', '-', '–', '|', '/');
        var parts = Regex.Split(text, Separator).Select(x => x.Trim()).Where(x => x.Length > 0).ToArray();
        return (text, quoted, parts);
    }

    public static string[] Queries(string rawTitle, string rawArtist)
    {
        var (whole, quoted, parts) = Clean(rawTitle);
        var artist = Artist(rawArtist);
        var flat = Regex.Replace(whole, Separator, " ").Trim();
        var withArtist = artist.Length > 0 && !Key(flat).Contains(Key(artist)) ? flat + " " + artist : flat;
        var longest = parts.Where(x => Key(x) != Key(artist)).OrderByDescending(x => x.Length).FirstOrDefault() ?? "";
        return new[] { quoted.Length > 0 ? quoted + " " + artist : withArtist, flat, longest }
            .Select(x => x.Trim()).Where(x => x.Length > 0).Distinct(StringComparer.OrdinalIgnoreCase).Take(3).ToArray();
    }

    public static async Task<IdentifiedSong?> FindAsync(string rawTitle, string rawArtist, double duration, CancellationToken token)
    {
        var (whole, quoted, parts) = Clean(rawTitle);
        var artist = Artist(rawArtist);
        var context = Key(rawTitle + " " + artist);
        var titles = parts.Append(whole).Append(quoted).Where(x => x.Length > 0).ToArray();
        foreach (var query in Queries(rawTitle, rawArtist))
        {
            token.ThrowIfCancellationRequested();
            using var response = await LyricHttp.Client.GetAsync("https://api.deezer.com/search?limit=10&q=" + Uri.EscapeDataString(query),
                HttpCompletionOption.ResponseHeadersRead, token);
            if (!response.IsSuccessStatusCode) return null;
            using var json = await LyricHttp.ReadJsonAsync(response, token);
            if (!json.RootElement.TryGetProperty("data", out var data) || data.ValueKind != JsonValueKind.Array) continue;
            IdentifiedSong? best = null; var bestScore = 0.0;
            foreach (var item in data.EnumerateArray().Take(10))
            {
                var song = Read(item);
                if (song is null) continue;
                var key = Key(song.Title);
                var titleScore = titles.Select(x => SongMatcher.Similarity(song.Title, x)).DefaultIfEmpty(0).Max();
                if (key.Length >= 2 && context.Contains(key)) titleScore = Math.Max(titleScore, 0.92);
                var artistKey = Key(song.Artist);
                var artistHit = SongMatcher.Similarity(song.Artist, artist) >= 0.8 || (artistKey.Length >= 2 && context.Contains(artistKey));
                // Tanpa artis yang cocok, judul pendek seperti "Lemon" terlalu mudah tertukar dengan lagu lain.
                if (titleScore < 0.8 || !artistHit) continue;
                // MV boleh lebih panjang (intro/outro), tetapi rekaman yang lebih panjang biasanya versi lain.
                var longer = duration > 0 && song.Duration > 0 && (song.Duration - duration > 10 || duration - song.Duration > 90);
                var otherVersion = Regex.Matches(song.Title + " " + song.Album, Version, Flags).Any(m => !Regex.IsMatch(rawTitle, Regex.Escape(m.Value), Flags));
                var score = titleScore - (longer ? 0.2 : 0) - (otherVersion ? 0.3 : 0);
                if (score > bestScore) { best = song; bestScore = score; } // Seri: urutan relevansi Deezer menang.
            }
            if (best is not null) return best;
        }
        return null;
    }

    private static IdentifiedSong? Read(JsonElement item)
    {
        static string Text(JsonElement e, string name)
        {
            if (e.ValueKind != JsonValueKind.Object || !e.TryGetProperty(name, out var v) || v.ValueKind != JsonValueKind.String) return "";
            var value = v.GetString() ?? "";
            return value[..Math.Min(value.Length, 256)];
        }
        if (item.ValueKind != JsonValueKind.Object) return null;
        var title = Text(item, "title_short"); if (title.Length == 0) title = Text(item, "title");
        var artist = item.TryGetProperty("artist", out var a) ? Text(a, "name") : "";
        var album = item.TryGetProperty("album", out var al) ? Text(al, "title") : "";
        var seconds = item.TryGetProperty("duration", out var d) && d.ValueKind == JsonValueKind.Number
            && d.TryGetDouble(out var value) && value is > 0 and <= 3600 ? value : 0;
        return title.Length == 0 || artist.Length == 0 ? null : new(title, artist, album, seconds);
    }

    [Conditional("DEBUG")]
    private static void SelfCheck()
    {
        var q = Queries("YOASOBI「アイドル」Official Music Video", "Ayase / YOASOBI");
        Debug.Assert(q[0].StartsWith("アイドル"), string.Join(" | ", q));
        q = Queries("【MV】米津玄師 - Lemon (歌詞付き)", "米津玄師");
        Debug.Assert(q.All(x => !x.Contains("MV") && !x.Contains("歌詞")) && q[0].Contains("Lemon"), string.Join(" | ", q));
        q = Queries("Tulus - Hati-Hati di Jalan (Official Lyric Video) #tulus", "TULUS");
        Debug.Assert(q[0] == "Tulus Hati-Hati di Jalan" && q.Contains("Hati-Hati di Jalan"), string.Join(" | ", q));
        q = Queries("Ed Sheeran - Shape of You [Official Video]", "Ed SheeranVEVO");
        Debug.Assert(q[0] == "Ed Sheeran Shape of You", string.Join(" | ", q));
    }
}

// Penyedia lirik memakai satu HttpClient dan membaca JSON langsung dari stream.
internal static class LyricHttp
{
    public const int MaxJsonBytes = 2 * 1024 * 1024;
    public static readonly HttpClient Client = CreateClient();

    private static HttpClient CreateClient()
    {
        var handler = new SocketsHttpHandler
        {
            AllowAutoRedirect = false,
            UseCookies = false,
            AutomaticDecompression = DecompressionMethods.GZip | DecompressionMethods.Deflate,
            MaxConnectionsPerServer = 2,
            MaxResponseHeadersLength = 32,
            PooledConnectionIdleTimeout = TimeSpan.FromSeconds(45),
            PooledConnectionLifetime = TimeSpan.FromMinutes(5)
        };
        var client = new HttpClient(handler) { Timeout = System.Threading.Timeout.InfiniteTimeSpan };
        client.DefaultRequestHeaders.UserAgent.ParseAdd($"NotchMusic/{AppInfo.Version} (personal Windows desktop project)");
        client.DefaultRequestHeaders.Accept.ParseAdd("application/json");
        return client;
    }

    public static async Task<JsonDocument> ReadJsonAsync(HttpResponseMessage response, CancellationToken token)
    {
        if (response.Content.Headers.ContentLength is long size && size > MaxJsonBytes)
            throw new LrcServiceException("Respons lirik terlalu besar; pencarian dihentikan agar memori tetap terbatas.");
        if (!(response.Content.Headers.ContentType?.MediaType ?? "").Contains("json", StringComparison.OrdinalIgnoreCase))
            throw new LrcServiceException("Layanan mengirim format selain JSON.");
        await using var body = await response.Content.ReadAsStreamAsync(token);
        using var limited = new LimitedReadStream(body, MaxJsonBytes);
        return await JsonDocument.ParseAsync(limited, new JsonDocumentOptions { MaxDepth = 32 }, token);
    }

    // Content-Length dapat kosong atau merupakan ukuran terkompresi: hitung byte yang benar-benar dibaca.
    private sealed class LimitedReadStream(Stream inner, int limit) : Stream
    {
        private int _read;
        public override bool CanRead => true;
        public override bool CanSeek => false;
        public override bool CanWrite => false;
        public override long Length => throw new NotSupportedException();
        public override long Position { get => _read; set => throw new NotSupportedException(); }
        private int Allow(int count) => Math.Min(count, limit - _read + 1);
        private int Count(int count)
        {
            _read += count;
            if (_read > limit) throw new LrcServiceException("Respons lirik melewati batas ukuran.");
            return count;
        }
        public override int Read(byte[] buffer, int offset, int count)
            => Count(inner.Read(buffer, offset, Allow(count)));
        public override int Read(Span<byte> buffer) => Count(inner.Read(buffer[..Allow(buffer.Length)]));
        public override async ValueTask<int> ReadAsync(Memory<byte> buffer, CancellationToken token = default)
            => Count(await inner.ReadAsync(buffer[..Allow(buffer.Length)], token));
        public override Task<int> ReadAsync(byte[] buffer, int offset, int count, CancellationToken token)
            => ReadAsync(buffer.AsMemory(offset, count), token).AsTask();
        public override void Flush() { }
        public override long Seek(long offset, SeekOrigin origin) => throw new NotSupportedException();
        public override void SetLength(long value) => throw new NotSupportedException();
        public override void Write(byte[] buffer, int offset, int count) => throw new NotSupportedException();
        // Pemilik respons yang menutup stream asli.
    }
}

internal static class LyricLimits
{
    public const int SourceCharacters = 64000;
    public const int CandidateCharacters = 128000;
    public const int Candidates = 8;
    public const int Lines = 2000;
    public const int TextFileCharacters = SourceCharacters + Lines * 32 + 4096;

    public static List<LyricCandidate> Retain(List<LyricCandidate> ranked)
    {
        var result = new List<LyricCandidate>(Candidates);
        var used = 0;
        foreach (var candidate in ranked)
        {
            var record = candidate.Record;
            var size = (record.PlainLyrics?.Length ?? 0) + (record.SyncedLyrics?.Length ?? 0)
                + (record.ParsedLines?.Sum(x => x.Text.Length) ?? 0);
            if (used + size > CandidateCharacters) continue;
            result.Add(candidate); used += size;
            if (result.Count == Candidates) break;
        }
        return result;
    }
}

internal sealed record BetterLyricsResult(LyricCandidate? Candidate, string Note);

internal static class BetterLyricsClient
{
    private static readonly SemaphoreSlim Gate = new(1, 1);
    private static DateTimeOffset _nextRequestUtc;
    // Satu hasil gagal saja, bukan cache katalog/riwayat lagu yang bertambah terus.
    private static string? _missKey;
    private static string _missNote = "";
    private static DateTimeOffset _missUntil;

    public static async Task<BetterLyricsResult> FindRomajiAsync(string title, string artist, string album,
        double duration, CancellationToken token)
    {
        if (string.IsNullOrWhiteSpace(title) || string.IsNullOrWhiteSpace(artist))
            return new(null, "Better Lyrics memerlukan judul dan artis.");
        album = album.Length > 512 ? album[..512] : album;
        var seconds = double.IsFinite(duration) && duration > 0
            ? (int)Math.Min(86400, Math.Round(duration, MidpointRounding.AwayFromZero)) : 0;
        var key = $"{title}\u001F{artist}\u001F{album}\u001F{seconds}";
        await Gate.WaitAsync(token);
        try
        {
            var now = DateTimeOffset.UtcNow;
            if (_missKey == key && now < _missUntil) return new(null, _missNote);
            if (now < _nextRequestUtc) return new(null, "Better Lyrics sedang meminta jeda; sumber lain dicoba.");
            BetterLyricsResult Miss(string note)
            {
                _missKey = key; _missNote = note; _missUntil = DateTimeOffset.UtcNow.AddSeconds(60);
                return new(null, note);
            }
            using var timeout = CancellationTokenSource.CreateLinkedTokenSource(token);
            timeout.CancelAfter(TimeSpan.FromSeconds(8));
            var path = "https://api.betterlyrics.org/getLyrics?s=" + Uri.EscapeDataString(title)
                + "&a=" + Uri.EscapeDataString(artist) + "&al=" + Uri.EscapeDataString(album)
                + "&d=" + seconds.ToString(CultureInfo.InvariantCulture);
            using var response = await LyricHttp.Client.GetAsync(path, HttpCompletionOption.ResponseHeadersRead, timeout.Token);
            if (response.StatusCode is HttpStatusCode.Unauthorized or HttpStatusCode.Forbidden)
                return Miss("Better Lyrics belum menyediakan hasil ini tanpa akses tambahan; sumber lain dicoba.");
            if (response.StatusCode is HttpStatusCode.TooManyRequests or HttpStatusCode.ServiceUnavailable)
            {
                var retry = response.Headers.RetryAfter;
                var delay = retry?.Delta ?? (retry?.Date is DateTimeOffset date
                    ? date - DateTimeOffset.UtcNow : TimeSpan.FromSeconds(60));
                if (delay < TimeSpan.FromSeconds(1)) delay = TimeSpan.FromSeconds(1);
                try { _nextRequestUtc = DateTimeOffset.UtcNow + delay; }
                catch (ArgumentOutOfRangeException) { _nextRequestUtc = DateTimeOffset.MaxValue; }
                return Miss("Better Lyrics sedang meminta jeda; sumber lain dicoba.");
            }
            if (response.StatusCode is HttpStatusCode.NotFound or HttpStatusCode.NoContent)
                return Miss("Romaji Better Lyrics belum tersedia; sumber lain dicoba.");
            if (!response.IsSuccessStatusCode)
                return Miss($"Better Lyrics belum dapat diakses (HTTP {(int)response.StatusCode}); sumber lain dicoba.");
            using var json = await LyricHttp.ReadJsonAsync(response, timeout.Token);
            if (json.RootElement.ValueKind != JsonValueKind.Object
                || !json.RootElement.TryGetProperty("ttml", out var value) || value.ValueKind != JsonValueKind.String)
                return Miss("Better Lyrics belum mengirim lirik berwaktu; sumber lain dicoba.");
            var ttml = value.GetString() ?? "";
            var lines = TtmlRomajiParser.Parse(ttml, duration, timeout.Token);
            if (lines.Length == 0)
                return Miss("Romaji berwaktu Better Lyrics belum lengkap atau belum cocok; sumber lain dicoba.");
            double? score = json.RootElement.TryGetProperty("score", out var match)
                && match.ValueKind == JsonValueKind.Number && match.TryGetDouble(out var number)
                && double.IsFinite(number) && number is >= 0 and <= 1 ? number : null;
            var record = new LrcRecord { Id = -1, TrackName = title, ArtistName = artist, AlbumName = album,
                Duration = duration, Source = "Better Lyrics", IsRomanized = true, ParsedLines = lines };
            // Cache hit tidak menyertakan score. Tidak tampilkan persentase kecocokan buatan.
            // API telah memilih satu lagu dari permintaan; score rendah tetap perlu pilihan manual.
            return new(new LyricCandidate(record, score ?? 1, score is null || score >= 0.90),
                "Romaji dan timing dari Better Lyrics. Periksa versi rekaman jika waktu atau teks berbeda.");
        }
        catch (OperationCanceledException) when (token.IsCancellationRequested) { throw; }
        catch (OperationCanceledException) { return new(null, "Better Lyrics melewati batas 8 detik; sumber lain dicoba."); }
        catch (Exception ex) when (ex is HttpRequestException or IOException or JsonException or System.Xml.XmlException or LrcServiceException)
        { return new(null, "Respons Better Lyrics belum bisa digunakan; sumber lain dicoba."); }
        finally { Gate.Release(); }
    }
}

// TTML: romaji ja-Latn memakai referensi baris asli; pembacaan asli dipakai untuk fallback Unison.
internal static class TtmlRomajiParser
{
    private const int MaxXmlCharacters = 384000;
    private const int MaxLineCharacters = 4096;
    private sealed record Entry(string Key, double? Begin, double? End, string Text);

    public static LyricLine[] Parse(string xml, double duration, CancellationToken token)
        => ParseCore(xml, duration, token, true);

    public static LyricLine[] ParseOriginal(string xml, double duration, CancellationToken token)
        => ParseCore(xml, duration, token, false);

    private static LyricLine[] ParseCore(string xml, double duration, CancellationToken token, bool requireRomaji)
    {
        if (xml.Length == 0 || xml.Length > MaxXmlCharacters) return [];
        var settings = new System.Xml.XmlReaderSettings { DtdProcessing = System.Xml.DtdProcessing.Prohibit,
            XmlResolver = null, IgnoreComments = true, MaxCharactersInDocument = MaxXmlCharacters };
        using var reader = System.Xml.XmlReader.Create(new StringReader(xml), settings);
        var romanized = new Dictionary<string, Entry>(StringComparer.Ordinal);
        var originals = new List<Entry>();
        var bodyDepth = -1;
        var originalChars = 0;
        var romajiChars = 0;
        var sawRomaji = false;
        while (reader.Read())
        {
            token.ThrowIfCancellationRequested();
            if (reader.Depth > 32) throw new LrcServiceException("Struktur TTML terlalu dalam.");
            if (reader.NodeType == System.Xml.XmlNodeType.EndElement && reader.LocalName == "body") bodyDepth = -1;
            if (reader.NodeType != System.Xml.XmlNodeType.Element) continue;
            if (reader.LocalName == "body") { bodyDepth = reader.Depth; continue; }
            if (requireRomaji && reader.LocalName == "transliteration" && !sawRomaji)
            {
                var language = reader.GetAttribute("lang", "http://www.w3.org/XML/1998/namespace") ?? "";
                if (!language.Equals("ja-Latn", StringComparison.OrdinalIgnoreCase)
                    && !language.StartsWith("ja-Latn-", StringComparison.OrdinalIgnoreCase)) continue;
                sawRomaji = true;
                using var section = reader.ReadSubtree();
                while (section.Read())
                {
                    token.ThrowIfCancellationRequested();
                    if (section.NodeType != System.Xml.XmlNodeType.Element || section.LocalName != "text") continue;
                    var entry = ReadEntry(section, token);
                    if (entry.Key.Length == 0 || entry.Text.Length == 0) continue;
                    romajiChars += entry.Text.Length;
                    if (romajiChars > LyricLimits.SourceCharacters || romanized.Count >= LyricLimits.Lines) return [];
                    if (!romanized.TryAdd(entry.Key, entry)) return [];
                }
            }
            else if (bodyDepth >= 0 && reader.LocalName == "p")
            {
                var entry = ReadEntry(reader, token);
                if (entry.Text.Length == 0) continue;
                originalChars += entry.Text.Length;
                if (originalChars > LyricLimits.SourceCharacters || originals.Count >= LyricLimits.Lines) return [];
                originals.Add(entry);
            }
        }
        if (originals.Count == 0 || (requireRomaji && (!sawRomaji || romanized.Count == 0))) return [];
        var picked = new List<Entry>(originals.Count);
        var matched = 0;
        var characters = 0;
        foreach (var original in originals)
        {
            token.ThrowIfCancellationRequested();
            Entry? romaji = null;
            var translated = requireRomaji && romanized.TryGetValue(original.Key, out romaji);
            // Lagu Jepang kadang memiliki baris Inggris. Boleh dipakai jika tidak mengandung aksara Jepang.
            if (requireRomaji && !translated && HasJapanese(original.Text)) return [];
            var text = translated ? romaji!.Text : original.Text;
            if (requireRomaji && HasJapanese(text)) return [];
            var begin = original.Begin ?? romaji?.Begin;
            var end = original.End ?? romaji?.End;
            if (begin is null || (end is double finish && finish < begin.Value)) return [];
            if (translated)
            {
                matched++;
                // Jangan menempelkan romaji yang merujuk baris/waktu berbeda.
                if (romaji!.Begin is double rb && rb < begin.Value - 2) return [];
                if (romaji.Begin is double late && end is double originalEnd && late > originalEnd + 2) return [];
                if (romaji.End is double early && early < begin.Value - 2) return [];
                if (romaji.End is double re && end is double oe && re > oe + 2) return [];
                if (romaji.Begin is double rBegin && romaji.End is double rEnd && rEnd < rBegin) return [];
            }
            if (duration > 0 && (end ?? begin.Value) > duration + 10) return [];
            characters += text.Length;
            if (characters > LyricLimits.SourceCharacters) return [];
            picked.Add(new Entry(original.Key, begin, end, text));
        }
        if (requireRomaji && matched == 0) return [];
        picked.Sort((a, b) => a.Begin!.Value.CompareTo(b.Begin!.Value));
        var output = new List<LyricLine>(Math.Min(LyricLimits.Lines, picked.Count * 2));
        for (var i = 0; i < picked.Count; i++)
        {
            var row = picked[i];
            var start = row.Begin!.Value;
            if (output.Count > 0 && Math.Abs(output[^1].Seconds - start) < 0.001)
                output[^1] = output[^1] with { Text = output[^1].Text + " / " + row.Text };
            else output.Add(new LyricLine(start, row.Text));
            var next = i + 1 < picked.Count ? picked[i + 1].Begin!.Value : double.PositiveInfinity;
            if (row.End is double end && end > start && next - end > 1.5)
                output.Add(new LyricLine(end, ""));
            if (output.Count > LyricLimits.Lines || output[^1].Text.Length > MaxLineCharacters) return [];
        }
        if (output.Any(x => x.Text.Length > MaxLineCharacters)
            || output.Sum(x => x.Text.Length + 3) > LyricLimits.SourceCharacters) return [];
        return output.ToArray();
    }

    private static Entry ReadEntry(System.Xml.XmlReader source, CancellationToken token)
    {
        var key = source.GetAttribute("for") ?? "";
        if (key.Length == 0 && source.HasAttributes)
        {
            while (source.MoveToNextAttribute())
                if (source.LocalName is "key" or "id") key = source.Value;
            source.MoveToElement();
        }
        var begin = ParseTime(source.GetAttribute("begin"));
        var end = ParseTime(source.GetAttribute("end"));
        double? firstSpan = null, lastSpan = null;
        var text = new StringBuilder();
        using var part = source.ReadSubtree();
        while (part.Read())
        {
            token.ThrowIfCancellationRequested();
            if (part.Depth > 32) throw new LrcServiceException("Struktur baris TTML terlalu dalam.");
            if (part.NodeType == System.Xml.XmlNodeType.Element && part.LocalName == "span")
            {
                if (ParseTime(part.GetAttribute("begin")) is double b) firstSpan = firstSpan is null ? b : Math.Min(firstSpan.Value, b);
                if (ParseTime(part.GetAttribute("end")) is double e) lastSpan = lastSpan is null ? e : Math.Max(lastSpan.Value, e);
            }
            if (part.NodeType is System.Xml.XmlNodeType.Text or System.Xml.XmlNodeType.CDATA
                or System.Xml.XmlNodeType.Whitespace or System.Xml.XmlNodeType.SignificantWhitespace)
            {
                if (text.Length + part.Value.Length > MaxLineCharacters)
                    throw new LrcServiceException("Baris lirik terlalu panjang.");
                text.Append(part.Value); // Spasi antarsuku kata dan tanda baca dipertahankan.
            }
            else if (part.NodeType == System.Xml.XmlNodeType.Element && part.LocalName == "br") text.Append(' ');
        }
        return new Entry(key, begin ?? firstSpan, end ?? lastSpan, text.ToString().Trim());
    }

    private static bool HasJapanese(string text)
    {
        foreach (var c in text)
            if (c is >= '\u3040' and <= '\u30ff' or >= '\u3400' and <= '\u9fff' or >= '\uff66' and <= '\uff9d') return true;
        return false;
    }

    internal static double? ParseTime(string? text)
    {
        if (string.IsNullOrWhiteSpace(text)) return null;
        text = text.Trim();
        var factor = 1.0;
        if (text.EndsWith("ms", StringComparison.Ordinal)) { factor = 0.001; text = text[..^2]; }
        else if (text.EndsWith('s')) text = text[..^1];
        var parts = text.Split(':');
        if (parts.Length is < 1 or > 3 || (factor != 1 && parts.Length != 1)) return null;
        var total = 0.0;
        for (var i = 0; i < parts.Length; i++)
        {
            if (!double.TryParse(parts[i], NumberStyles.AllowDecimalPoint, CultureInfo.InvariantCulture, out var n)
                || !double.IsFinite(n) || n < 0 || (i > 0 && n >= 60)) return null;
            total = total * 60 + n;
        }
        total *= factor;
        return total <= 86400 ? total : null;
    }
}

// Unison membaca LRC/TTML komunitas dan mengubah teks pada baris yang sama menjadi romaji.
internal static class UnisonClient
{
    private static readonly SemaphoreSlim Gate = new(1, 1);
    private static DateTimeOffset _nextRequestUtc;
    public static bool HasJapaneseText(string text)
        => Regex.IsMatch(text, @"[\u3040-\u30ff\u3400-\u9fff\uff66-\uff9d]");
    public static bool HasJapanese(LyricDocument document)
        => document.IsSynced ? document.Lines.Any(x => HasJapaneseText(x.Text)) : HasJapaneseText(document.PlainText);
    public static bool IsJapanese(LrcRecord record)
    {
        var language = record.Language.Trim().Replace('_', '-');
        if (language.Equals("ja", StringComparison.OrdinalIgnoreCase) || language.StartsWith("ja-", StringComparison.OrdinalIgnoreCase)) return true;
        if (language.Length > 0 && !language.Equals("und", StringComparison.OrdinalIgnoreCase)
            && !language.Equals("unknown", StringComparison.OrdinalIgnoreCase)) return false;
        // Kanji saja juga dapat merupakan bahasa Mandarin; Otomatis tidak menyamakannya dengan Jepang.
        return record.ParsedLines is { } lines ? lines.Any(x => LrcClient.HasKana(x.Text))
            : LrcClient.HasKana(record.SyncedLyrics ?? record.PlainLyrics ?? "");
    }
    private static string Text(JsonElement element, string name, int max = 512)
        => element.TryGetProperty(name, out var value) && value.ValueKind == JsonValueKind.String
            && value.GetString() is string text && text.Length <= max ? text : "";

    private static LrcRecord? ParseRecord(JsonElement value, CancellationToken token, bool metadataOnly = false, bool preferRomaji = true)
    {
        if (value.ValueKind != JsonValueKind.Object) return null;
        var title = Text(value, "song");
        if (string.IsNullOrWhiteSpace(title)) return null;
        var record = new LrcRecord { TrackName = title, ArtistName = Text(value, "artist"),
            AlbumName = Text(value, "album"), Language = Text(value, "language", 32), Source = "Unison" };
        if (value.TryGetProperty("id", out var id) && id.ValueKind == JsonValueKind.Number && id.TryGetInt64(out var number)) record.Id = number;
        if (value.TryGetProperty("duration", out var duration) && duration.ValueKind == JsonValueKind.Number
            && duration.TryGetDouble(out var seconds) && double.IsFinite(seconds) && seconds is > 0 and <= 86400)
            record.Duration = seconds;
        if (metadataOnly) return record;
        var format = Text(value, "format", 16).ToLowerInvariant();
        var lyrics = Text(value, "lyrics", format == "ttml" ? 384000 : LyricLimits.SourceCharacters);
        if (lyrics.Length == 0) return null;
        if (format == "ttml")
        {
            record.ParsedLines = preferRomaji ? TtmlRomajiParser.Parse(lyrics, record.Duration, token) : [];
            record.IsRomanized = record.ParsedLines.Length > 0;
            if (!record.IsRomanized) record.ParsedLines = TtmlRomajiParser.ParseOriginal(lyrics, record.Duration, token);
            if (record.ParsedLines.Length == 0) return null;
        }
        else if (format == "lrc") record.SyncedLyrics = lyrics;
        else if (format == "plain") record.PlainLyrics = lyrics;
        else return null;
        var document = LyricDocument.From(record);
        if (document.Preview.Length == 0) return null;
        if (record.Language.StartsWith("ja-Latn", StringComparison.OrdinalIgnoreCase)
            && !HasJapanese(document)) record.IsRomanized = true;
        return record;
    }

    private static IEnumerable<JsonElement> Records(JsonDocument? document)
    {
        if (document is null || document.RootElement.ValueKind != JsonValueKind.Object
            || !document.RootElement.TryGetProperty("success", out var ok) || ok.ValueKind != JsonValueKind.True
            || !document.RootElement.TryGetProperty("data", out var data)) yield break;
        if (data.ValueKind == JsonValueKind.Object) yield return data;
        else if (data.ValueKind == JsonValueKind.Array)
            foreach (var item in data.EnumerateArray().Take(5)) yield return item;
    }

    public static async Task<List<LrcRecord>> FindAsync(string title, string artist, string album,
        double duration, bool preferRomaji, CancellationToken token)
    {
        var records = new List<LrcRecord>(5);
        if (!string.IsNullOrWhiteSpace(artist))
        {
            var path = "/lyrics?song=" + Uri.EscapeDataString(title) + "&artist=" + Uri.EscapeDataString(artist);
            if (album.Length > 0) path += "&album=" + Uri.EscapeDataString(album);
            if (duration is > 0 and <= 86400) path += "&duration=" + duration.ToString("0.###", CultureInfo.InvariantCulture);
            using var exact = await RequestAsync(path, null, token);
            foreach (var value in Records(exact))
                if (ParseRecord(value, token, preferRomaji: preferRomaji) is LrcRecord record) records.Add(record);
            if (records.Count > 0) return records;
        }
        // Search hanya berisi metadata. Ambil badan lirik melalui ID hasilnya, maksimal tiga kandidat.
        List<LyricCandidate> candidates;
        using (var search = await RequestAsync("/lyrics/search?limit=5&q=" + Uri.EscapeDataString(title + " " + artist), null, token))
        {
            var metadata = Records(search).Select(x => ParseRecord(x, token, true)).OfType<LrcRecord>();
            candidates = SongMatcher.Rank(metadata, title, artist, 0).Take(3).ToList();
        }
        foreach (var candidate in candidates)
        {
            if (candidate.Record.Id <= 0) continue;
            using var full = await RequestAsync("/lyrics/" + candidate.Record.Id.ToString(CultureInfo.InvariantCulture), null, token);
            foreach (var value in Records(full))
                if (ParseRecord(value, token, preferRomaji: preferRomaji) is LrcRecord record) records.Add(record);
        }
        return records;
    }

    public static async Task<LrcRecord> RomanizeAsync(LrcRecord source, LyricDocument document, CancellationToken token)
    {
        if (!document.IsSynced) throw new LrcServiceException("Romaji sinkron memerlukan lirik Jepang berwaktu.");
        var positions = Enumerable.Range(0, document.Lines.Length)
            .Where(i => HasJapaneseText(document.Lines[i].Text)).ToArray();
        if (positions.Length == 0) return source;
        if (positions.Length > 400 || positions.Any(i => document.Lines[i].Text.Length > 1000))
            throw new LrcServiceException("Teks terlalu besar untuk konversi otomatis. Muat file romaji .lrc yang sudah siap.");
        var output = document.Lines.ToArray();
        var chars = output.Sum(x => x.Text.Length + 3);
        var cursor = 0;
        var requests = 0;
        while (cursor < positions.Length)
        {
            token.ThrowIfCancellationRequested();
            if (++requests > 4) throw new LrcServiceException("Konversi melewati batas empat kelompok. Muat file romaji .lrc.");
            var start = cursor;
            var batch = new List<string>(100);
            var batchChars = 0;
            while (cursor < positions.Length && batch.Count < 100)
            {
                var line = document.Lines[positions[cursor]].Text;
                if (batchChars + line.Length > 12000) break;
                batch.Add(line); batchChars += line.Length; cursor++;
            }
            using var json = await RequestAsync("/translate", batch, token);
            if (json is null || json.RootElement.ValueKind != JsonValueKind.Object
                || !json.RootElement.TryGetProperty("lines", out var lines) || lines.ValueKind != JsonValueKind.Array
                || lines.GetArrayLength() != batch.Count)
                throw new LrcServiceException("Jumlah baris romaji tidak cocok. Hasil konversi tidak dipakai agar timing tidak tertukar.");
            var language = Text(json.RootElement, "detectedLang", 32);
            if (language.Length > 0 && !language.Equals("ja", StringComparison.OrdinalIgnoreCase))
                throw new LrcServiceException("Bahasa hasil konversi bukan Jepang. Periksa pilihan lagunya.");
            var item = 0;
            foreach (var line in lines.EnumerateArray())
            {
                var romaji = line.ValueKind == JsonValueKind.Object ? Text(line, "romanization", 4096).Trim() : "";
                // Jangan memakai kolom translation: itu terjemahan Inggris, bukan bacaan romaji.
                if (romaji.Length == 0 || HasJapaneseText(romaji) || !Regex.IsMatch(romaji, "[A-Za-z]")
                    || romaji.Contains('\n') || romaji.Contains('\r'))
                    throw new LrcServiceException("Romaji dari Unison belum lengkap. Hasil sebagian tidak disimpan; coba lagi nanti atau Muat file .lrc.");
                var index = positions[start + item++];
                chars += romaji.Length - output[index].Text.Length;
                if (chars > LyricLimits.SourceCharacters) throw new LrcServiceException("Hasil romaji terlalu besar.");
                // Waktu dan urutan tidak diubah. Baris Inggris, kosong, dan jeda instrumental juga tetap.
                output[index] = output[index] with { Text = romaji };
            }
        }
        return new LrcRecord { Id = source.Id, TrackName = source.TrackName, ArtistName = source.ArtistName,
            AlbumName = source.AlbumName, Duration = source.Duration, Source = source.Source + " · romaji Unison",
            IsRomanized = true, MachineRomanized = true, IsLocal = source.IsLocal, Language = "ja-Latn", ParsedLines = output };
    }

    private static async Task<JsonDocument?> RequestAsync(string path, List<string>? lines, CancellationToken token)
    {
        await Gate.WaitAsync(token);
        try
        {
            var wait = _nextRequestUtc - DateTimeOffset.UtcNow;
            if (wait > TimeSpan.FromSeconds(2))
                throw new LrcServiceException($"Unison meminta jeda sekitar {Math.Ceiling(wait.TotalSeconds)} detik. Lirik lokal tetap bisa dimuat.");
            if (wait > TimeSpan.Zero) await Task.Delay(wait, token);
            using var timeout = CancellationTokenSource.CreateLinkedTokenSource(token);
            timeout.CancelAfter(TimeSpan.FromSeconds(lines is null ? 8 : 15));
            using var request = new HttpRequestMessage(lines is null ? HttpMethod.Get : HttpMethod.Post,
                "https://unison.boidu.dev" + path);
            if (lines is not null)
                request.Content = new StringContent(JsonSerializer.Serialize(new { lines, to = "en", from = "ja" }), Encoding.UTF8, "application/json");
            using var response = await LyricHttp.Client.SendAsync(request, HttpCompletionOption.ResponseHeadersRead, timeout.Token);
            if (response.StatusCode == HttpStatusCode.NotFound && lines is null) return null;
            if (response.StatusCode is HttpStatusCode.TooManyRequests or HttpStatusCode.ServiceUnavailable)
            {
                var retry = response.Headers.RetryAfter;
                var delay = retry?.Delta ?? (retry?.Date is DateTimeOffset date ? date - DateTimeOffset.UtcNow : TimeSpan.FromSeconds(30));
                if (delay < TimeSpan.FromSeconds(1)) delay = TimeSpan.FromSeconds(1);
                try { _nextRequestUtc = DateTimeOffset.UtcNow + delay; }
                catch (ArgumentOutOfRangeException) { _nextRequestUtc = DateTimeOffset.MaxValue; }
                throw new LrcServiceException("Unison sedang sibuk atau meminta jeda. Hasil yang sudah tersimpan tetap dapat digunakan.");
            }
            if (!response.IsSuccessStatusCode)
                throw new LrcServiceException($"Unison belum dapat memproses lirik (HTTP {(int)response.StatusCode}). Coba lagi nanti atau Muat file .lrc.");
            return await LyricHttp.ReadJsonAsync(response, timeout.Token);
        }
        catch (OperationCanceledException) when (!token.IsCancellationRequested)
        { throw new LrcServiceException("Unison melewati batas waktu. Coba lagi nanti atau Muat file .lrc."); }
        finally
        {
            var earliest = DateTimeOffset.UtcNow.AddMilliseconds(500);
            if (_nextRequestUtc < earliest) _nextRequestUtc = earliest;
            Gate.Release();
        }
    }
}

// Romaji lokal: bacaan kanji dari penganalisis fonetik Jepang bawaan Windows,
// lalu kana diubah ke romaji Hepburn. Tanpa internet; waktu tiap baris tidak berubah.
internal static class LocalRomaji
{
    private const int AnalyzerLimit = 100; // Batas input JapanesePhoneticAnalyzer.GetWords.
    private static readonly Regex JapaneseRun = new(@"[぀-ヿ㐀-鿿豈-﫿ｦ-ﾝ々〆ヶ]+", RegexOptions.Compiled);

    public static LrcRecord Romanize(LrcRecord source, LyricDocument document, CancellationToken token)
    {
        if (!document.IsSynced) throw new LrcServiceException("Romaji sinkron memerlukan lirik Jepang berwaktu.");
        var output = document.Lines.ToArray();
        var converted = 0;
        for (var i = 0; i < output.Length; i++)
        {
            token.ThrowIfCancellationRequested();
            var text = output[i].Text;
            if (!UnisonClient.HasJapaneseText(text)) continue;
            var romaji = Line(text);
            if (romaji.Length == 0 || UnisonClient.HasJapaneseText(romaji))
                throw new LrcServiceException("Sebagian baris belum bisa dibaca oleh penganalisis Jepang Windows.");
            output[i] = output[i] with { Text = romaji }; // Hanya teks yang diganti; detik baris tetap.
            converted++;
        }
        if (converted == 0) return source;
        return new LrcRecord { Id = source.Id, TrackName = source.TrackName, ArtistName = source.ArtistName,
            AlbumName = source.AlbumName, Duration = source.Duration, Source = source.Source + " · romaji lokal",
            IsRomanized = true, MachineRomanized = true, IsLocal = source.IsLocal, Language = "ja-Latn", ParsedLines = output };
    }

    // Satu baris lirik: bagian Jepang dibaca per kata, bagian lain (Inggris, angka, tanda baca) dipertahankan.
    internal static string Line(string text)
    {
        var words = new List<string>();
        var last = 0;
        foreach (Match run in JapaneseRun.Matches(text))
        {
            AddOther(words, text[last..run.Index]);
            foreach (var chunk in Chunks(run.Value))
            {
                var phonemes = global::Windows.Globalization.JapanesePhoneticAnalyzer.GetWords(chunk, false);
                foreach (var phoneme in phonemes)
                {
                    var display = phoneme.DisplayText ?? "";
                    var yomi = string.IsNullOrEmpty(phoneme.YomiText) ? display : phoneme.YomiText;
                    // Partikel ditulis menurut bunyinya: は = wa, へ = e, を = wo.
                    var romaji = !phoneme.IsPhraseStart && display == "は" ? "wa"
                        : !phoneme.IsPhraseStart && display == "へ" ? "e"
                        : display == "を" ? "wo"
                        : Kana(yomi);
                    if (romaji.Length == 0) continue;
                    // Konsonan ganda (っ) di akhir kata digabung dengan kata berikutnya.
                    if (words.Count > 0 && words[^1].EndsWith('\u0001'))
                        words[^1] = words[^1][..^1] + (romaji.StartsWith("ch") ? "t"
                            : "aiueo".Contains(romaji[0]) ? "" : romaji[0].ToString()) + romaji;
                    else words.Add(romaji);
                }
            }
            last = run.Index + run.Length;
        }
        AddOther(words, text[last..]);
        var line = string.Join(" ", words).Replace("\u0001", "");
        line = Regex.Replace(line, @"\s+([,.!?~)\]」』])", "$1");
        line = Regex.Replace(line, @"([(\[「『])\s+", "$1");
        return Regex.Replace(line, @"\s{2,}", " ").Trim();
    }

    private static void AddOther(List<string> words, string text)
    {
        text = text.Replace('　', ' ').Replace("、", ", ").Replace("。", ". ").Replace("！", "! ").Replace("？", "? ")
            .Replace("～", "~").Replace("〜", "~").Replace("・", " ").Replace("（", "(").Replace("）", ")")
            .Replace("「", "\"").Replace("」", "\"").Replace("『", "\"").Replace("』", "\"").Replace("…", "...");
        foreach (var part in text.Split(' ', StringSplitOptions.RemoveEmptyEntries)) words.Add(part);
    }

    private static IEnumerable<string> Chunks(string run)
    {
        for (var i = 0; i < run.Length; i += AnalyzerLimit)
            yield return run.Substring(i, Math.Min(AnalyzerLimit, run.Length - i));
    }

    private static readonly Dictionary<string, string> Table = BuildTable();

    private static Dictionary<string, string> BuildTable()
    {
        var t = new Dictionary<string, string>(StringComparer.Ordinal);
        void Row(string kana, params string[] romaji) { for (var i = 0; i < kana.Length; i++) t[kana[i].ToString()] = romaji[i]; }
        Row("あいうえお", "a", "i", "u", "e", "o");
        Row("かきくけこ", "ka", "ki", "ku", "ke", "ko"); Row("がぎぐげご", "ga", "gi", "gu", "ge", "go");
        Row("さしすせそ", "sa", "shi", "su", "se", "so"); Row("ざじずぜぞ", "za", "ji", "zu", "ze", "zo");
        Row("たちつてと", "ta", "chi", "tsu", "te", "to"); Row("だぢづでど", "da", "ji", "zu", "de", "do");
        Row("なにぬねの", "na", "ni", "nu", "ne", "no");
        Row("はひふへほ", "ha", "hi", "fu", "he", "ho"); Row("ばびぶべぼ", "ba", "bi", "bu", "be", "bo");
        Row("ぱぴぷぺぽ", "pa", "pi", "pu", "pe", "po");
        Row("まみむめも", "ma", "mi", "mu", "me", "mo");
        Row("やゆよ", "ya", "yu", "yo");
        Row("らりるれろ", "ra", "ri", "ru", "re", "ro");
        Row("わゐゑを", "wa", "wi", "we", "o");
        Row("ぁぃぅぇぉゃゅょゎ", "a", "i", "u", "e", "o", "ya", "yu", "yo", "wa");
        t["ゔ"] = "vu"; t["ゕ"] = "ka"; t["ゖ"] = "ke";
        // Gabungan yōon: き+ゃ = kya, し+ゃ = sha, dst.
        var yoon = new (string Kana, string Head)[] { ("き", "ky"), ("ぎ", "gy"), ("し", "sh"), ("じ", "j"), ("ち", "ch"),
            ("ぢ", "j"), ("に", "ny"), ("ひ", "hy"), ("び", "by"), ("ぴ", "py"), ("み", "my"), ("り", "ry") };
        foreach (var (kana, head) in yoon)
        { t[kana + "ゃ"] = head + "a"; t[kana + "ゅ"] = head + "u"; t[kana + "ょ"] = head + "o"; }
        t["しぇ"] = "she"; t["じぇ"] = "je"; t["ちぇ"] = "che"; t["てぃ"] = "ti"; t["でぃ"] = "di"; t["とぅ"] = "tu"; t["どぅ"] = "du";
        t["ふぁ"] = "fa"; t["ふぃ"] = "fi"; t["ふぇ"] = "fe"; t["ふぉ"] = "fo"; t["ふゅ"] = "fyu";
        t["うぃ"] = "wi"; t["うぇ"] = "we"; t["うぉ"] = "wo"; t["つぁ"] = "tsa"; t["つぃ"] = "tsi"; t["つぇ"] = "tse"; t["つぉ"] = "tso";
        t["ゔぁ"] = "va"; t["ゔぃ"] = "vi"; t["ゔぇ"] = "ve"; t["ゔぉ"] = "vo"; t["いぇ"] = "ye"; t["くぁ"] = "kwa";
        return t;
    }

    // Kana (hiragana/katakana) ke romaji Hepburn. っ menggandakan konsonan, ー memanjangkan vokal,
    // dan ん sebelum vokal/y diberi apostrof agar tidak terbaca salah (kan'i, bukan kani).
    internal static string Kana(string input)
    {
        var text = new StringBuilder(input.Length);
        foreach (var c in input) text.Append(c is >= 'ァ' and <= 'ヶ' ? (char)(c - 0x60) : c); // Katakana -> hiragana.
        var s = text.ToString();
        var result = new StringBuilder(s.Length * 2);
        var doubleNext = false;
        for (var i = 0; i < s.Length; i++)
        {
            var c = s[i];
            if (c == 'っ') { doubleNext = true; continue; }
            if (c == 'ん')
            {
                result.Append('n');
                if (i + 1 < s.Length && Table.TryGetValue(s[i + 1].ToString(), out var after) && after.Length > 0 && "aiueoy".Contains(after[0]))
                    result.Append('\'');
                continue;
            }
            if (c is 'ー' or 'ｰ')
            {
                var vowel = result.Length > 0 ? result[^1] : '\0';
                if ("aiueo".Contains(vowel)) result.Append(vowel);
                continue;
            }
            string? romaji = null;
            if (i + 1 < s.Length && Table.TryGetValue(s.Substring(i, 2), out var pair)) { romaji = pair; i++; }
            else if (Table.TryGetValue(c.ToString(), out var single)) romaji = single;
            if (romaji is null)
            {
                // Tanda kana tanpa bunyi (mis. ・ atau ゛ terpisah) dibuang; karakter lain dipertahankan.
                if (!char.IsWhiteSpace(c) && c is not (>= '\u3040' and <= '\u30ff')) result.Append(c);
                doubleNext = false; continue;
            }
            if (doubleNext)
            {
                if (romaji.StartsWith("ch")) result.Append('t');
                else if (!"aiueo".Contains(romaji[0])) result.Append(romaji[0]);
                doubleNext = false;
            }
            result.Append(romaji);
        }
        // っ di akhir kata (mis. "あっ" atau kata yang terpotong analyzer) disambung ke kata berikutnya.
        if (doubleNext) result.Append('\u0001');
        return result.ToString();
    }
}

// Satu dokumen aktif di RAM. Lagu-lagu lain tetap berupa file kecil di disk, tanpa indeks katalog di RAM.
internal sealed class CachedLyrics
{
    public CachedLyrics() { }
    public int Schema { get; set; } = 1;
    public string Identity { get; set; } = "";
    public LyricsMode Mode { get; set; }
    public double PlaybackDuration { get; set; }
    public double OffsetSeconds { get; set; }
    public string Title { get; set; } = "";
    public string Artist { get; set; } = "";
    public string Album { get; set; } = "";
    public string Source { get; set; } = "";
    public string Language { get; set; } = "";
    public double Duration { get; set; }
    public bool Instrumental { get; set; }
    public bool IsRomanized { get; set; }
    public bool MachineRomanized { get; set; }
    public LyricLine[] Lines { get; set; } = [];
    public string PlainText { get; set; } = "";

    public static CachedLyrics Create(string identity, LyricsMode mode, double playbackDuration, double offset,
        LrcRecord record, LyricDocument document) => new()
    {
        Identity = identity, Mode = mode, PlaybackDuration = playbackDuration, OffsetSeconds = offset,
        Title = record.TrackName, Artist = record.ArtistName, Album = record.AlbumName, Source = record.Source,
        Language = record.Language, Duration = record.Duration, Instrumental = record.Instrumental,
        IsRomanized = LrcClient.IsMarkedRomaji(record), MachineRomanized = record.MachineRomanized,
        Lines = document.Lines, PlainText = document.IsSynced ? "" : document.PlainText
    };

    public LrcRecord ToRecord() => new() { TrackName = Title, ArtistName = Artist, AlbumName = Album,
        Duration = Duration, Instrumental = Instrumental, Source = Source, Language = Language,
        IsRomanized = IsRomanized, MachineRomanized = MachineRomanized, IsLocal = true,
        ParsedLines = Lines, PlainLyrics = PlainText };

    public bool IsValid()
    {
        if (Schema != 1 || Identity is null || Identity.Length is < 1 or > 4096
            || (int)Mode is < 0 or > 2
            || !double.IsFinite(OffsetSeconds) || Math.Abs(OffsetSeconds) > 120
            || !double.IsFinite(PlaybackDuration) || PlaybackDuration is < 0 or > 86400
            || !double.IsFinite(Duration) || Duration is < 0 or > 86400
            || new[] { Title, Artist, Album, Source, Language }.Any(x => x is null || x.Length > 512)
            || Lines is null || Lines.Length > LyricLimits.Lines || PlainText is null
            || PlainText.Length > LyricLimits.SourceCharacters || (Lines.Length > 0 && PlainText.Length > 0)) return false;
        var previous = -1.0;
        var chars = 0;
        foreach (var line in Lines)
        {
            if (line is null || !double.IsFinite(line.Seconds) || line.Seconds is < 0 or > 86400
                || line.Seconds <= previous || line.Text is null || line.Text.Length > 4096
                || line.Text.Contains('\n') || line.Text.Contains('\r')) return false;
            previous = line.Seconds; chars += line.Text.Length + 3;
            if (chars > LyricLimits.SourceCharacters) return false;
        }
        return Instrumental || (Lines.Length > 0 ? Lines.Any(x => !string.IsNullOrWhiteSpace(x.Text)) : !string.IsNullOrWhiteSpace(PlainText));
    }
}

// Ringkasan cache tidak memuat Lines/PlainText. Cache lama tetap dibaca dengan skema yang sama.
internal sealed class LyricCacheHeader
{
    public LyricCacheHeader() { }
    public int Schema { get; set; } = 1;
    public string Identity { get; set; } = "";
    public LyricsMode Mode { get; set; }
    public double PlaybackDuration { get; set; }
    public double Duration { get; set; }
    public string Title { get; set; } = "";
    public string Artist { get; set; } = "";
    public string Album { get; set; } = "";
    public bool IsRomanized { get; set; }
    [System.Text.Json.Serialization.JsonIgnore]
    public double RecordingDuration => PlaybackDuration > 0 ? PlaybackDuration : Duration;

    public static LyricCacheHeader From(CachedLyrics entry) => new()
    {
        Schema = entry.Schema, Identity = entry.Identity, Mode = entry.Mode,
        PlaybackDuration = entry.PlaybackDuration, Duration = entry.Duration,
        Title = entry.Title, Artist = entry.Artist, Album = entry.Album, IsRomanized = entry.IsRomanized
    };
    public bool IsValid() => Schema == 1 && Identity is { Length: > 0 and <= 4096 }
        && (int)Mode is >= 0 and <= 2 && Identity.Split('\u001f').Length == 3
        && new[] { Title, Artist, Album }.All(x => x is not null && x.Length <= 512)
        && double.IsFinite(PlaybackDuration) && PlaybackDuration is >= 0 and <= 86400
        && double.IsFinite(Duration) && Duration is >= 0 and <= 86400;
}

internal sealed record CacheTrackNames(string Title, string Artist, string ArtistKey, string[] Titles, string Version);
internal sealed record CacheSongMatch(double Score, bool Exact);

internal static class CacheSongMatcher
{
    private const string VersionWords = @"\b(?:live|remix|cover|acoustic|instrumental|karaoke|remaster(?:ed)?|version|ver|edit|radio|extended|demo|session|piano|sped\s*up|speed\s*up|slowed|nightcore|8d|mono|stereo|re[- ]?recorded|part|pt|vol|volume|movement|chapter|feat|ft)\b|ライブ|カバー|リミックス|アレンジ";
    private const RegexOptions Flags = RegexOptions.IgnoreCase | RegexOptions.CultureInvariant;

    private static string CleanArtist(string artist)
    {
        artist = SongMatcher.CleanArtist(artist);
        return Regex.Replace(artist, @"(?:\s*(?:[-–—|]\s*)?VEVO|\s+(?:[-–—|]\s*)?(?:Official(?:\s+(?:YouTube\s+)?Channel)?|Topic|Topik))\s*$", "", Flags).Trim();
    }
    private static string Key(string text)
        => new(text.Normalize(NormalizationForm.FormKC).Where(char.IsLetterOrDigit).Select(char.ToLowerInvariant).ToArray());

    public static CacheTrackNames Names(string title, string artist)
    {
        artist = CleanArtist(artist);
        (title, artist) = SongMatcher.CleanQuery(title, artist);
        // Hanya label kemasan video yang dibuang. Label versi rekaman tetap diperiksa di bawah.
        title = Regex.Replace(title, @"[\(\[【](?:\s*(?:official\s+)?(?:lyrics?|lyric\s+video|audio|music\s+video|video|mv|pv|visuali[sz]er|hd|4k|\d{3,4}p)\s*)[\)\]】]", " ", Flags);
        title = Regex.Replace(title, @"\s+[-–—|]\s*(?:lyrics?|audio|visuali[sz]er)\s*$", "", Flags).Trim();
        title = Regex.Replace(title, @"[\(\[【]\s*[\)\]】]", "").Trim();
        var aliases = new List<string> { title };
        foreach (Match part in Regex.Matches(title, @"[\(（]([^\)）]+)[\)）]").Cast<Match>().Take(4))
        {
            var inside = part.Groups[1].Value.Trim();
            var outside = title.Remove(part.Index, part.Length).Trim();
            // Alias beda aksara harus tertulis di metadata; jangan membuang subjudul/nomor bagian sembarang.
            if (!Regex.IsMatch(title, VersionWords, Flags) && inside.Length > 0 && outside.Length > 0
                && UnisonClient.HasJapaneseText(inside) != UnisonClient.HasJapaneseText(outside))
            { aliases.Add(inside); aliases.Add(outside); }
        }
        var keys = aliases.Select(Key).Where(x => x.Length > 0).Distinct().Take(6).ToArray();
        return new(title, artist, Key(artist), keys, VersionKey(title));
    }
    public static CacheTrackNames FromIdentity(string identity)
    {
        var parts = identity.Split('\u001f');
        return Names(parts.ElementAtOrDefault(0) ?? "", parts.ElementAtOrDefault(1) ?? "");
    }

    private static string VersionKey(string title)
    {
        if (!Regex.IsMatch(title, VersionWords, Flags)) return "";
        var labels = new List<string>();
        foreach (Match part in Regex.Matches(title, @"[\(\[【（]([^\)\]】）]+)[\)\]】）]"))
            if (Regex.IsMatch(part.Groups[1].Value, VersionWords, Flags)) labels.Add(Key(part.Groups[1].Value));
        var tail = Regex.Match(title, @"\s[-–—|]\s(?<tail>.+)$");
        if (tail.Success && Regex.IsMatch(tail.Groups["tail"].Value, VersionWords, Flags)) labels.Add(Key(tail.Groups["tail"].Value));
        labels.AddRange(Regex.Matches(title, VersionWords, Flags).Cast<Match>().Select(x => Key(x.Value)));
        labels.AddRange(Regex.Matches(title, @"\b(?:19|20)\d{2}\b").Cast<Match>().Select(x => x.Value));
        return string.Join("|", labels.Distinct().OrderBy(x => x, StringComparer.Ordinal));
    }

    private static double TitleSimilarity(CacheTrackNames first, CacheTrackNames second)
    {
        if (first.Titles.Intersect(second.Titles, StringComparer.Ordinal).Any()) return 1;
        // Fuzzy sangat ketat untuk variasi ejaan Latin. Aksara Jepang memakai alias yang benar-benar tersimpan.
        if (UnisonClient.HasJapaneseText(first.Title) || UnisonClient.HasJapaneseText(second.Title)) return 0;
        var best = 0.0;
        foreach (var a in first.Titles)
            foreach (var b in second.Titles)
                if (a.Length >= 8 && b.Length >= 8
                    && Regex.Matches(a, @"\d+").Cast<Match>().Select(x => x.Value)
                        .SequenceEqual(Regex.Matches(b, @"\d+").Cast<Match>().Select(x => x.Value)))
                    best = Math.Max(best, SongMatcher.Similarity(a, b));
        return best;
    }

    public static CacheSongMatch? Match(LyricCacheHeader cached, CacheTrackNames requested, LyricsMode mode, double duration)
    {
        if (!cached.IsValid() || (mode != LyricsMode.Auto && cached.Mode != mode) || requested.ArtistKey.Length == 0
            || !double.IsFinite(duration) || duration <= 0 || cached.RecordingDuration <= 0
            || Math.Abs(duration - cached.RecordingDuration) > 3) return null;
        var playback = FromIdentity(cached.Identity);
        var canonical = Names(cached.Title, cached.Artist);
        var storedVersion = playback.Version.Length > 0 ? playback.Version : canonical.Version;
        if (!string.Equals(requested.Version, storedVersion, StringComparison.Ordinal)) return null;
        var score = 0.0;
        foreach (var candidate in new[] { playback, canonical })
            if (requested.ArtistKey == candidate.ArtistKey)
                score = Math.Max(score, TitleSimilarity(requested, candidate));
        return score >= 0.96 ? new(score, score == 1) : null;
    }

    // Dipakai hanya untuk urutan pilihan manual; tidak pernah menjadi keputusan otomatis.
    public static double SuggestionScore(LyricCacheHeader cached, CacheTrackNames requested, LyricsMode mode, double duration)
    {
        var score = 0.0;
        foreach (var candidate in new[] { FromIdentity(cached.Identity), Names(cached.Title, cached.Artist) })
        {
            var artist = requested.ArtistKey.Length > 0 && requested.ArtistKey == candidate.ArtistKey ? 1
                : SongMatcher.Similarity(requested.Artist, candidate.Artist);
            score = Math.Max(score, 0.7 * TitleSimilarity(requested, candidate) + 0.3 * artist);
        }
        if (duration > 0 && cached.RecordingDuration > 0 && Math.Abs(duration - cached.RecordingDuration) <= 3) score += 0.15;
        if (mode == cached.Mode) score += 0.05;
        return score;
    }
}

internal static class LyricCache
{
    private static readonly SemaphoreSlim Gate = new(1, 1);
    private static readonly JsonSerializerOptions Options = new() { MaxDepth = 12 };
    private const int HeaderBytes = 64 * 1024;
    private static string DirectoryPath => Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "NotchMusic", "Lyrics");
    private static string HeaderDirectory => Path.Combine(DirectoryPath, "lookup-v1");
    public static string Identity(string title, string artist, string album)
    {
        // Pertahankan format v4.5 agar semua file lama tetap dapat ditemukan persis.
        (title, artist) = SongMatcher.CleanQuery(title, artist);
        static string Normalize(string value) => Regex.Replace(value[..Math.Min(value.Length, 512)]
            .Normalize(NormalizationForm.FormKC).Trim(), @"\s+", " ").ToUpperInvariant();
        return Normalize(title) + "\u001f" + Normalize(artist) + "\u001f" + Normalize(album);
    }
    private static string FilePath(string identity, LyricsMode mode)
    {
        var key = Encoding.UTF8.GetBytes(identity + "\n" + (int)mode);
        return Path.Combine(DirectoryPath, Convert.ToHexString(System.Security.Cryptography.SHA256.HashData(key)) + ".json");
    }
    private static bool IsCacheName(string name) => Regex.IsMatch(name, @"\A[0-9A-Fa-f]{64}\.json\z", RegexOptions.CultureInvariant);
    private static bool ReadFailure(Exception ex) => ex is IOException or UnauthorizedAccessException or JsonException or ArgumentException;

    private static async Task<CachedLyrics?> ReadFileAsync(string path, CancellationToken token)
    {
        try
        {
            if (!File.Exists(path)) return null;
            await using var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read, 4096, true);
            if (stream.Length > LyricFiles.MaxBytes) return null;
            var entry = await JsonSerializer.DeserializeAsync<CachedLyrics>(stream, Options, token);
            return entry is not null && entry.IsValid()
                && Path.GetFileName(FilePath(entry.Identity, entry.Mode)) == Path.GetFileName(path) ? entry : null;
        }
        catch (Exception ex) when (ReadFailure(ex)) { return null; }
    }

    private static async Task WriteHeaderQuietlyAsync(string path, LyricCacheHeader header)
    {
        try
        {
            Directory.CreateDirectory(HeaderDirectory);
            var json = JsonSerializer.Serialize(header, Options);
            if (Encoding.UTF8.GetByteCount(json) <= HeaderBytes)
                await LyricFiles.WriteAtomicAsync(Path.Combine(HeaderDirectory, Path.GetFileName(path)), json);
        }
        catch (Exception ex) when (ReadFailure(ex)) { Debug.WriteLine("Ringkasan cache belum dapat disimpan."); }
    }

    private static async Task<LyricCacheHeader?> ReadHeaderAsync(string path, CancellationToken token)
    {
        try
        {
            var data = new FileInfo(path);
            if (!data.Exists || data.Length > LyricFiles.MaxBytes) return null;
            var summary = new FileInfo(Path.Combine(HeaderDirectory, data.Name));
            LyricCacheHeader? header = null;
            if (summary.Exists && summary.Length <= HeaderBytes && summary.LastWriteTimeUtc >= data.LastWriteTimeUtc)
            {
                try
                {
                    await using var stream = new FileStream(summary.FullName, FileMode.Open, FileAccess.Read, FileShare.Read, 4096, true);
                    header = await JsonSerializer.DeserializeAsync<LyricCacheHeader>(stream, Options, token);
                }
                catch (Exception ex) when (ReadFailure(ex)) { }
                if (header?.IsValid() == true && Path.GetFileName(FilePath(header.Identity, header.Mode)) == data.Name) return header;
            }
            // Migrasi ringan: deserialisasi metadata saja, tanpa membuat array objek Lines dari cache lama.
            await using (var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read, 4096, true))
            {
                if (stream.Length > LyricFiles.MaxBytes) return null;
                header = await JsonSerializer.DeserializeAsync<LyricCacheHeader>(stream, Options, token);
            }
            if (header?.IsValid() != true || Path.GetFileName(FilePath(header.Identity, header.Mode)) != data.Name) return null;
            token.ThrowIfCancellationRequested();
            await WriteHeaderQuietlyAsync(path, header);
            return header;
        }
        catch (Exception ex) when (ReadFailure(ex)) { return null; }
    }

    private sealed record Candidate(string Path, LyricCacheHeader Header, CacheSongMatch Match, DateTime Modified);

    public static async Task<CachedLyrics?> ReadAsync(string identity, LyricsMode mode, double duration, CancellationToken token)
    {
        await Gate.WaitAsync(token);
        try
        {
            // Otomatis tetap dapat membaca simpanan v4.5 dari mode Romaji/Asli; tidak perlu mengunduh ulang.
            var modes = mode == LyricsMode.Auto ? new[] { LyricsMode.Auto, LyricsMode.Romaji, LyricsMode.Original } : new[] { mode };
            var exactPaths = modes.Select(m => FilePath(identity, m)).ToArray();
            for (var i = 0; i < modes.Length; i++)
            {
                token.ThrowIfCancellationRequested();
                var exact = await ReadFileAsync(exactPaths[i], token);
                if (exact is not null && exact.Identity == identity && exact.Mode == modes[i]
                    && (duration <= 0 || exact.PlaybackDuration <= 0 || Math.Abs(duration - exact.PlaybackDuration) <= 3)) return exact;
            }
            if (!Directory.Exists(DirectoryPath) || duration <= 0) return null;
            var requested = CacheSongMatcher.FromIdentity(identity);
            Candidate? best = null;
            var ambiguous = false;
            foreach (var path in Directory.EnumerateFiles(DirectoryPath, "*.json", SearchOption.TopDirectoryOnly))
            {
                token.ThrowIfCancellationRequested();
                if (!IsCacheName(Path.GetFileName(path)) || exactPaths.Contains(path)) continue;
                var header = await ReadHeaderAsync(path, token);
                if (header is null || CacheSongMatcher.Match(header, requested, mode, duration) is not CacheSongMatch match) continue;
                var candidate = new Candidate(path, header, match, File.GetLastWriteTimeUtc(path));
                if (best is null) { best = candidate; continue; }
                var equivalent = match.Exact && best.Match.Exact
                    && Math.Abs(header.RecordingDuration - best.Header.RecordingDuration) <= 1;
                if (equivalent)
                {
                    // Beberapa sumber dapat sudah menyimpan rekaman yang sama; pilih koreksi/hasil yang terakhir disimpan.
                    if ((candidate.Header.Mode == mode && best.Header.Mode != mode)
                        || ((candidate.Header.Mode == mode) == (best.Header.Mode == mode) && candidate.Modified > best.Modified)) best = candidate;
                }
                else if (match.Score - best.Match.Score >= 0.03) { best = candidate; ambiguous = false; }
                else if (best.Match.Score - match.Score < 0.03)
                {
                    ambiguous = true;
                    if (match.Score > best.Match.Score) best = candidate;
                }
            }
            if (best is null || ambiguous) return null;
            var saved = await ReadFileAsync(best.Path, token);
            // Periksa lagi badan file yang dipilih: ringkasan lama/rusak tidak boleh menghubungkan lagu keliru.
            return saved is not null && CacheSongMatcher.Match(LyricCacheHeader.From(saved), requested, mode, duration) is not null
                ? saved : null;
        }
        catch (Exception ex) when (ReadFailure(ex)) { return null; }
        finally { Gate.Release(); }
    }

    public static async Task<List<LyricCandidate>> ListChoicesAsync(string title, string artist, LyricsMode mode,
        double duration, CancellationToken token)
    {
        await Gate.WaitAsync(token);
        try
        {
            if (!Directory.Exists(DirectoryPath)) return [];
            var requested = CacheSongMatcher.Names(title, artist);
            var top = new List<(string Name, LyricCacheHeader Header, double Score, DateTime Modified)>(9);
            foreach (var path in Directory.EnumerateFiles(DirectoryPath, "*.json", SearchOption.TopDirectoryOnly))
            {
                token.ThrowIfCancellationRequested();
                var name = Path.GetFileName(path);
                if (!IsCacheName(name)) continue;
                var header = await ReadHeaderAsync(path, token);
                if (header is null) continue;
                var score = CacheSongMatcher.SuggestionScore(header, requested, mode, duration);
                top.Add((name, header, score, File.GetLastWriteTimeUtc(path)));
                top.Sort((a, b) => a.Score != b.Score ? b.Score.CompareTo(a.Score) : b.Modified.CompareTo(a.Modified));
                if (top.Count > LyricLimits.Candidates) top.RemoveAt(top.Count - 1);
            }
            // Hanya ringkasan untuk menu; teks lengkap baru dibaca setelah pengguna memilih.
            return top.Select(x => new LyricCandidate(new LrcRecord { TrackName = x.Header.Title, ArtistName = x.Header.Artist,
                AlbumName = x.Header.Album, Duration = x.Header.RecordingDuration, Source = "Lirik tersimpan", IsLocal = true,
                IsRomanized = x.Header.IsRomanized, CacheFileName = x.Name }, x.Score, false)).ToList();
        }
        catch (Exception ex) when (ReadFailure(ex)) { return []; }
        finally { Gate.Release(); }
    }

    public static async Task<CachedLyrics?> ReadChosenAsync(string fileName, CancellationToken token)
    {
        if (!IsCacheName(fileName)) return null;
        await Gate.WaitAsync(token);
        try { return await ReadFileAsync(Path.Combine(DirectoryPath, fileName), token); }
        finally { Gate.Release(); }
    }

    public static async Task<bool> WriteAsync(CachedLyrics entry)
    {
        if (!entry.IsValid()) return false;
        await Gate.WaitAsync();
        string? temporary = null;
        try
        {
            Directory.CreateDirectory(DirectoryPath);
            var path = FilePath(entry.Identity, entry.Mode);
            temporary = path + "." + Guid.NewGuid().ToString("N") + ".tmp";
            await using (var stream = new FileStream(temporary, FileMode.CreateNew, FileAccess.Write, FileShare.None, 4096, true))
            {
                await JsonSerializer.SerializeAsync(stream, entry, Options);
                await stream.FlushAsync();
                if (stream.Length > LyricFiles.MaxBytes) return false;
            }
            File.Move(temporary, path, true);
            await WriteHeaderQuietlyAsync(path, LyricCacheHeader.From(entry));
            return true;
        }
        catch (Exception ex) when (ReadFailure(ex)) { Debug.WriteLine("Cache lirik belum dapat disimpan."); return false; }
        finally { LyricFiles.DeleteTemporary(temporary); Gate.Release(); }
    }
}

internal static class LyricFiles
{
    public const int MaxBytes = 512 * 1024;
    private static string Header(string value) => new string(value.Take(512)
        .Select(c => char.IsControl(c) || c == ']' ? ' ' : c).ToArray()).Trim();
    public static string FileName(LrcRecord record)
    {
        var name = "NotchMusic - " + record.ArtistName + " - " + record.TrackName
            + (LrcClient.IsMarkedRomaji(record) ? " (romaji)" : "");
        var invalid = Path.GetInvalidFileNameChars();
        return new string(name.Take(140).Select(c => invalid.Contains(c) || char.IsControl(c) ? '_' : c).ToArray()).TrimEnd(' ', '.');
    }
    public static string Export(LrcRecord record, LyricDocument document, double offset)
    {
        if (!document.IsSynced) return document.PlainText;
        var text = new StringBuilder();
        text.AppendLine("[ti:" + Header(record.TrackName) + "]");
        text.AppendLine("[ar:" + Header(record.ArtistName) + "]");
        text.AppendLine("[al:" + Header(record.AlbumName) + "]");
        text.AppendLine("[by:" + Header(record.Source) + "]");
        text.AppendLine($"[re:NotchMusic v{AppInfo.Version}]");
        text.AppendLine("[notch-romaji:" + (LrcClient.IsMarkedRomaji(record) ? "1" : "0") + "]");
        text.AppendLine("[notch-auto-romaji:" + (record.MachineRomanized ? "1" : "0") + "]");
        // LRC offset positif memajukan lirik; koreksi positif di UI menundanya.
        text.AppendLine("[offset:" + Math.Round(-offset * 1000).ToString("0", CultureInfo.InvariantCulture) + "]");
        foreach (var line in document.Lines)
        {
            var ms = (long)Math.Round(line.Seconds * 1000, MidpointRounding.AwayFromZero);
            text.Append('[').Append((ms / 60000).ToString("00", CultureInfo.InvariantCulture)).Append(':')
                .Append((ms / 1000 % 60).ToString("00", CultureInfo.InvariantCulture)).Append('.')
                .Append((ms % 1000).ToString("000", CultureInfo.InvariantCulture)).Append(']').AppendLine(line.Text);
        }
        return text.ToString();
    }
    public static async Task<LrcRecord> ReadAsync(string path, string title, string artist, LyricsMode mode, CancellationToken token)
    {
        var extension = Path.GetExtension(path).ToLowerInvariant();
        if (extension is not ".lrc" and not ".txt") throw new LrcServiceException("Pilih file .lrc atau .txt.");
        await using var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read, 4096, true);
        if (stream.Length > MaxBytes) throw new LrcServiceException("File lirik maksimal 512 KiB.");
        using var reader = new StreamReader(stream, new UTF8Encoding(false, true), true, 4096, true);
        var text = await reader.ReadToEndAsync(token);
        if (text.Length > LyricLimits.TextFileCharacters) throw new LrcServiceException("Isi file lirik melewati batas ukuran.");
        string Tag(string name)
        {
            var match = Regex.Match(text, @"(?m)^\[" + Regex.Escape(name) + @":([^\]\r\n]{0,512})\]\r?$");
            return match.Success ? match.Groups[1].Value.Trim() : "";
        }
        var record = new LrcRecord { TrackName = title, ArtistName = artist, Source = "File lokal", IsLocal = true };
        if (extension == ".lrc")
        {
            if (Tag("ti").Length > 0) record.TrackName = Tag("ti");
            if (Tag("ar").Length > 0) record.ArtistName = Tag("ar");
            record.AlbumName = Tag("al");
            if (double.TryParse(Tag("offset"), NumberStyles.Integer, CultureInfo.InvariantCulture, out var offsetMs)
                && double.IsFinite(offsetMs) && Math.Abs(offsetMs) <= 120000)
            {
                record.ImportedOffsetSeconds = -offsetMs / 1000;
                // Simpan koreksi terpisah agar waktu negatif tidak terlipat menjadi baris 00:00 yang sama.
                text = Regex.Replace(text, @"(?im)^\[offset:[^\]\r\n]*\]\r?$", "[offset:0]");
            }
            record.SyncedLyrics = text;
            record.IsRomanized = Tag("notch-romaji") == "1";
            record.MachineRomanized = record.IsRomanized && Tag("notch-auto-romaji") == "1";
        }
        else record.PlainLyrics = text;
        var document = LyricDocument.From(record);
        if (document.Preview.Length == 0 || (extension == ".lrc" && !document.IsSynced))
            throw new LrcServiceException("File kosong atau timestamp LRC tidak valid. Contoh format: [00:12.500]Teks baris.");
        // Teks Latin bisa berupa bahasa Indonesia/Inggris; pilihan mode bukan bukti bahwa teks itu romaji.
        return record;
    }
    public static async Task WriteAtomicAsync(string path, string content)
    {
        if (Encoding.UTF8.GetByteCount(content) > MaxBytes) throw new LrcServiceException("File hasil terlalu besar untuk disimpan.");
        var temporary = path + "." + Guid.NewGuid().ToString("N") + ".tmp";
        try
        {
            await File.WriteAllTextAsync(temporary, content, new UTF8Encoding(false));
            File.Move(temporary, path, true);
        }
        finally { DeleteTemporary(temporary); }
    }
    public static void DeleteTemporary(string? path)
    {
        if (path is null) return;
        try { File.Delete(path); } catch (IOException) { } catch (UnauthorizedAccessException) { }
    }
}
