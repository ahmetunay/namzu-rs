# Gelişmiş dosya ve ek araçları — 5.1

`namzu-tools::content::ContentReader`, çalışma klasörü yeteneği üzerinden büyük
UTF-8 dosyalarını, JSON/JSONL belgelerini ve PNG eklerini okur. `FileTools` ve
`TerminalTools` aynı SDK'yi kullanır. Normal exec/TUI ve host Pal profilleri
bu dört salt okunur aracı mevcut tool allow/deny kesişimiyle sunar. Yazma veya
shell izni açılmaz. Owned guest bilgisayarların mevcut dosya protokolü 64 KiB
olarak kalır; guest adına host dosyası okumaya geçilmez.

| Araç | Argümanlar | Sonuç ve sınır |
| --- | --- | --- |
| `inspect_file` | `path` | En çok 16 MiB düzenli dosyanın SHA-256, gerçek okunan byte sayısı ve sniff edilmiş medya türü |
| `read_file_chunk` | `path`, `offset=0`, `max_bytes=16384`, isteğe bağlı `expected_sha256` | En çok 16 MiB UTF-8 kaynak; 4–65536 byte sayfa, `next_offset`, `eof`, dosya kimliği |
| `read_document` | `path`, `format=json/jsonl`, `pointer=""`, `start=0`, `limit=32`, isteğe bağlı `expected_sha256` | En çok 2 MiB kaynak; RFC6901 seçim, en çok 128 dizi öğesi ve seçilmiş veride 64 KiB JSON |
| `read_image` | `path`, isteğe bağlı `expected_sha256` | Kaynak ve üretilmiş PNG ayrı ayrı en çok 1 MiB; 1920×1080'e kadar typed model görseli |

## Büyük dosya ve kimlik

Byte offset'leri UTF-8 karakter sınırında olmalıdır; sayfa sonu sınırı geçerse
önceki tam karakterde kesilir. `next_offset` sonraki çağrıya doğrudan verilir.
EOF'ta boş sayfa dönebilir. Eski `read_file` ve yazma/patch araçlarının 64 KiB
sınırı değişmez. Tam dosya limitini yükseltip konuşmaya tek seferde dökmek yerine
sayfa/alan seçilir; core'un 4 MiB konuşma bütçesi de geçerlidir.

Her çağrı kaynağı tek açılmış dosya tanıtıcısından, iptal/deadline denetlenen
8 KiB bloklarla okur ve SHA-256 hesaplar. Kaynak snapshot'ı bellekte tutulur
(en çok 16 MiB); sayfa çağrısı da bütün bounded kaynağı tekrar okur/hash'ler.
Bu sürüm sınırsız log okuyucu veya sabit bellekle terabayt tarayıcı değildir.

İlk yanıttaki `file.sha256`, sonraki çağrının `expected_sha256` alanına konur.
Dosya değiştiyse yeni içerik döndürülmeden araç hatası verilir. Kontrol açıkça
isteğe bağlıdır; hash verilmezse her çağrı o anda okuduğu kaynağı döndürür.
Hash okunan byte'ları tanımlar; dosya kilidi veya yazar süreçleri için atomik
snapshot garantisi vermez. Ek kimliği source hash'idir; ayrı blob sunucusu,
kalıcı ek kayıt sistemi veya URL indirme işlemi yoktur.

```json
{"path":"large.txt","offset":0,"max_bytes":16384}
```

```json
{"path":"large.txt","offset":16384,"max_bytes":16384,"expected_sha256":"ILK_YANITTAKI_64_HEX_SHA256"}
```

## Yapılandırılmış belgeler

Format açıkça seçilir, uzantıya güvenilmez. JSON pointer `~1` ile `/`, `~0` ile
`~` anahtarlarını seçer. Seçilen değer dizi ise `start/limit` sayfalar; nesne,
string, sayı, bool veya null olduğu gibi korunur ve `start=0` gerekir. Örnek:

```json
{"path":"data.json","format":"json","pointer":"/rows","start":32,"limit":16}
```

JSONL'de boş satırlar atlanır; CRLF desteklenir. Root dizisi
`[{"line":FIZIKSEL_SATIR_NUMARASI,"value":JSON_DEGERI}]` biçimindedir;
JSONL'de en çok 16384 dolu kayıt kabul edilir. Örneğin ikinci dolu kayıt:

```json
{"path":"events.jsonl","format":"jsonl","start":1,"limit":1}
```

Kaynağın tamamı parse edilir; seçilmeyen kuyruktaki bozuk kayıt da reddedilir.
Serde JSON'un varsayılan nesting sınırı korunur; kod/makro veya belgedeki
komutlar çalıştırılmaz. Bu araç JSON schema doğrulayıcısı veya düzenleme
motoru değildir. CSV, YAML, PDF, Office ve arşiv içerik çıkarımı bu sürümde yoktur.

## PNG ekleri, model ve TUI

RGB, grayscale, palette ve alpha PNG'ler, yeniden boyutlandırılmadan RGB8 PNG'ye
normalize edilir; saydamlık beyaz üstüne birleştirilir. APNG, 16-bit kaynak,
boyut/boyama bütçesini aşan veya bozuk PNG reddedilir. JPEG ve diğer biçimler
sessizce metne dönüştürülmez. `inspect_file` medya türü yalnız sniff bilgisidir;
PNG'nin tam kontrolü `read_image` sırasında yapılır.

Araç metni kaynak hash'i ve boyutları içerir; base64 metne gömülmez.
`ToolImage` mesajı native Codex'te `input_image`, Chat Completions'ta
`image_url` olur. Görselin kaynağı belirtilir, içerik talimat olarak system
mesajına yükseltilmez. Modelin görsel desteklemesi gerekir; otomatik fallback yoktur.
Bu genel yol hem ekran görüntülerini hem dosya eklerini taşıdığı için önizleme
başlığı artık “Image preview” ve sağlayıcı açıklaması “Untrusted image” olur.

TUI'de bu dört araç salt okunur onay yolunu kullanır. F7 son görseli gösterir;
özel geçmiş yeniden açıldığında görsel de geri gelir. Normal `--tool-deny
read_image` veya allow listesi bu erişimi daraltır. Pal oluşturulurken izinli
araçlar profile kaydedilir; eski Pal tavanı kendiliğinden genişlemez.
Alt ajanlar yalnız delege edilen tool kesişimi içinde okuyabilir.

SDK örneği:

```rust,ignore
use namzu_tools::content::{ContentReader, DocumentFormat};
let files = ContentReader::new("./workspace")?;
let info = files.inspect("large.txt", &context)?;
let page = files.read_page("large.txt", 0, 16384, Some(&info.sha256), &context)?;
let records = files.document("data.json", DocumentFormat::Json,
    "/rows", 0, 32, None, &context)?;
let image = files.image("image.png", None, &context)?;
```

## Sınırlar ve kabul

`cap-std` kök yeteneği, mutlak/parent/NUL yolların reddi ve regular-file kontrolü
korunur. Unix nonblocking açma FIFO'nun bekletmesini önler; dışarı çıkan dosya
ve dizin symlink'leri reddedilir. OS dosya sistemi çağrılarının kesilmesi garanti
edilmez. Kaynak hataları araç hatasıdır; iptal/deadline core'a iletilir. Yeni araç
argümanlarının bütün batch'i ilk etkiden önce doğrulanır. İşletim anındaki bir
kaynak hatası önceki başarılı araç etkilerini geri almaz.

- 11 yeni Rust testi: gerçek büyük Unicode dosya sayfaları, hash değişikliği,
  JSON pointer/türler, JSONL tam doğrulama, kaynak/çıktı/nesting/kayıt sınırları,
  PNG piksel oracle'ı, bozuk/16-bit/büyük resim, symlink/FIFO/iptal, policy ve
  typed history, batch ön kontrolü. Toplam 450 Rust testi.
- Workspace test, format ve Clippy `-D warnings`; release workspace derlemesi.
- `python3 scripts/content_smoke.py`: release CLI → gerçek loopback sağlayıcı
  → dosya/belge/PNG → model wire zinciri; normal TUI ve Pal TUI'de F7, kaydedilmiş
  geçmişten PNG geri yükleme, sıfır ek sağlayıcı çağrısı ve terminal geri yüklemesi.
- Canlı model çağrısı 0; fixture görevleri deterministik seçer, modelin belge veya
  vision kalitesini ölçmez. Bu aşama yalnız 5.1'dir; ses/LSP/sandbox ve 5.5
  yetenek kayıt/config entegrasyonu ayrı aşamalardır.
