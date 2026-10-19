# Ses SDK ve açık CLI — 5.2

`namzu-audio`, bağımsız ve isteğe bağlı Rust ses paketidir. Core/ajan döngüsü ses
paketine bağımlı değildir. Paket immutable WAV verisi, `SpeechInput` ve
`SpeechOutput` sürücü sözleşmeleri, `transcribe`/`synthesize` akışları, içerik
barındırmayan SDK olayları ve çalışma klasöründe güvenli dosya erişimi sağlar.
`namzu-provider::speech::SpeechClient`, OpenAI uyumlu ses HTTP uçlarını uygular.

## Format ve bütçeler

| Veri | Kabul edilen sözleşme |
| --- | --- |
| Ses | RIFF/WAVE, PCM signed 16-bit little-endian; 1 veya 2 kanal, 8000–48000 Hz |
| Süre | Kanal başına örnek/frame sayısıyla tam olarak en çok 60 saniye |
| Boyut | Girdi ve çıktı ayrı ayrı en çok 12 MiB, en çok 128 RIFF chunk |
| Metinden sese | En çok 4000 Unicode karakter ve 16 KiB UTF-8; boş/NUL metin reddedilir |
| Transkript | En çok 64 KiB UTF-8; JSON HTTP yanıtı en çok 128 KiB |
| Süre aşımı | CLI varsayılan 300 saniye; açık `--timeout` 1–86400 |

RIFF uzunluğu, chunk sınırları/padding, fmt/data tekilliği, PCM codec'i,
byte rate, block alignment ve data/frame hizası doğrulanır. Boş ses, MP3,
FLAC/OGG, float PCM, WAV extensible, RF64/RIFX ve diğer bit derinlikleri bu
sürümde yoktur. Süre tahmini dosya boyutuna veya sunucunun bildirdiği süreye
dayanmaz; gerçek PCM data uzunluğundan hesaplanır. `AudioInfo` örnekleme hızı,
kanallar, frame/süre, byte sayısı ve SHA-256 içerir.

Yerel dosya parser'ı bildirilmiş uzunluklarda katıdır. HTTP sentez sürücüsü,
yalnız **tam ve bounded HTTP gövdesi** alındıktan sonra RIFF `0xffffffff`
bilinmeyen uzunluk işaretini gerçek boyuta dönüştürebilir. Data uzunluğu da
`0xffffffff` ise kalan gövde son data chunk'ıdır. Ardından bütün WAV ve süre
kontrolleri yeniden uygulanır. Boyut dışında header veya PCM verisi tahmin
edilmez; yeniden örnekleme, decode-convert veya sessiz kısaltma yapılmaz.
SHA-256 kaydedilecek canonical WAV byte'larını tanımlar.

## CLI

Yerel inceleme anahtar veya servis istemez:

```sh
namzu-rs audio --cwd ./project inspect input.wav
```

Transkripsiyon komutu açıkça seçilen WAV dosyasını servise gönderir:

```sh
namzu-rs audio --cwd ./project --timeout 300 transcribe input.wav \
  --model STT_MODEL --language tr \
  --base-url https://api.openai.com/v1 --api-key-env SPEECH_API_KEY
```

Sentez metni servise gönderir ve **yeni** bir WAV oluşturur:

```sh
namzu-rs audio --cwd ./project synthesize 'Merhaba dünya' \
  --output answer.wav --voice alloy --model TTS_MODEL \
  --base-url https://api.openai.com/v1 --api-key-env SPEECH_API_KEY
```

`SPEECH_API_KEY` seçtiğin ortam değişkenidir; anahtar komut satırında verilmez.
Varsayılan selector `OPENAI_API_KEY` olsa da SDK/CLI anahtarı keşfetmez veya
Codex abonelik girişinden kopyalamaz. Model ve voice, bağladığın servisin
kabul ettiği değerler olmalıdır. Bu uçlar API/local-server ses hizmetleridir;
Codex aboneliğine ses kotası dahil olduğu iddiası yoktur. Canlı hizmetin ücreti
kendi hesabına/modeline bağlıdır; bu aşamanın testlerinde ücretli hizmet yoktur.

Güvendiğin yerel uyumlu servis için loopback HTTP kullanılabilir; örneğin
`--base-url http://127.0.0.1:PORT/v1`. Remote HTTP, URL içine yazılmış kimlik
bilgileri, query ve fragment reddedilir. Ortam proxy/CA ayarları ve TLS
doğrulaması korunur. Yerel servis anahtar istemiyorsa bile bu adaptörde açık
bir env selector üzerinden uygun placeholder seçmek gerekir.

Tüm `audio` komutları stdout'ta `format_version/status/output` veya `error`
JSON döndürür. Transkripsiyon çıktısı kullanıcıya istenen metni içerir;
`events` alanı ses/text payload'ı içermez. Ham ses base64 olarak stdout'a veya
ajan geçmişine gömülmez. Dosya yolları `--cwd` köküne göredir.

## Sürücü ve olay sözleşmesi

`SpeechInput::transcribe(&Wav, language, context)` → `Transcript`;
`SpeechOutput::synthesize(text, voice, context)` → doğrulanmış `Wav`.
SDK wrapper'ları çağrıdan önce argüman/iptal kontrolü, çağrıdan sonra
sonuç/iptal kontrolü yapar. Transkriptler untrusted kullanıcı verisidir;
SDK bunları otomatik system talimatı veya ajan görevi yapmaz.

Olaylar ayrı typed `AudioEvent` sözleşmesidir; mevcut core `Event` enum'u veya
mesaj biçimi değiştirilmez. `AudioEvents` sink'i senkron ve sınırlı çalışmalıdır.

- Transkripsiyon: `Started → InputValidated → Completed`; hata/iptal: `Failed`.
- Sentez: `Started → Completed`; hata/iptal: `Failed`.
- Olaylarda yalnız işlem türü, doğrulanmış ses metadata'sı ve metnin byte sayısı
  bulunur. Transcript/WAV Debug çıktıları da payload'ı göstermez.
- Sentez `Completed`, sürücünün doğrulanmış ses üretmesini belirtir; dosya
  publication işlemi ayrı `AudioWorkspace::publish` adımıdır.

SDK örneği (uygulama ortamında açıkça seçilmiş anahtar):

```rust,ignore
use namzu_audio::{AudioWorkspace, transcribe, synthesize};
use namzu_provider::speech::SpeechClient;
let workspace = AudioWorkspace::new("./project")?;
let audio = workspace.read("input.wav", &context)?;
let mut driver = SpeechClient::new(BASE_URL, &key, STT_MODEL, TTS_MODEL)?;
let mut events = Vec::new();
let transcript = transcribe(&mut driver, &audio, Some("tr"), &context, &mut events)?;
workspace.check_destination("answer.wav")?;
let spoken = synthesize(&mut driver, &transcript.text, "alloy", &context, &mut events)?;
workspace.publish("answer.wav", &spoken, &context)?;
```

## HTTP ve dosya güvenliği

Girdi WAV ve dil etiketi ağ isteğinden önce doğrulanır. Transkripsiyon
`/audio/transcriptions` multipart isteği model/language/response_format ve
`audio/wav` dosyası gönderir. Filename sabit `input.wav` olur; yerel dosya yolu
sunucuya taşınmaz. Multipart delimiter çakışması reddedilir. Sentez
`/audio/speech` JSON isteğinde `response_format=wav` ister.

HTTP, mevcut iptal edilebilir async transport'un senkron SDK cephesini kullanır;
bekleyen istek/gövde iptalde düşürülür, arka planda kopmuş bir worker kalmaz.
Yönlendirme, uygulama düzeyinde retry ve kimlik bilgisi keşfi yapılmaz. Hata
body'leri veya anahtarlar loglanmaz. Socket'in kapanması uzaktaki inference'ın
kesildiğini veya harcanmış kotanın geri alınacağını garanti etmez. Başarısız veya
belirsiz çağrı otomatik tekrar edilmez; bu sürümde kalıcı audio request journal'ı yoktur.

`cap-std` workspace sınırı, relative-path/parent/NUL kontrolü ve regular-file
kontrolü vardır. Unix nonblocking açma FIFO'da asılmayı önler. Çıktı önce aynı
parent'ta private 0600 geçici dosyaya yazılıp sync edilir; create-new hard link
publication mevcut dosya/symlink'i değiştirmez. Ön kontrolden sonra oluşan dosya
yarışı da overwrite yapmaz. İptal/yazma hatasında geçici dosya temizlenir.
OS dosya sistemi çağrılarının iptalle kesilmesi ve crash sonrası dizin entry
kalıcılığı garanti edilmez. Çıktının varlığı mümkün olduğunca servis çağrısından
önce kontrol edilir; geç ortaya çıkan disk/izin yarışı servis maliyetini geri alamaz.

## Kabul ve kalan entegrasyon

- 23 yeni Rust testi: 12 SDK, 8 gerçek HTTP, 3 gerçek CLI süreç testi. Workspace
  toplam 473 test; format, Clippy `-D warnings`, release workspace derlemesi.
- `python3 scripts/audio_smoke.py`: sekiz gerçek release CLI/loopback kontrolü;
  offline inceleme, exact multipart, Unicode transcript, exact private WAV,
  ön kontrolle sıfır ağ isteği, uzun yanıtın reddi, redakte hata/no replay,
  gerçek SIGINT ve deadline'da HTTP bağlantısının kapanması, geçici dosya temizliği.
- Canlı model/inference çağrısı 0. Fixture bilinen PCM byte'ları ve sabit metin
  döndürür; gerçek STT/TTS doğruluğu, ses kalitesi veya sağlayıcı hesabı ölçülmez.

Mikrofon kaydı, hoparlörden playback, gerçek zamanlı ses akışı ve codec dönüşümü
yoktur. TUI/Pal ajan araçlarına otomatik dosya gönderme izni eklenmez; ses
capability/config/izin bağlantıları 5.5 ile hazırdır
([açık seçim ve izinler](CAPABILITIES.md)); masaüstü ses UI'ı 8'e bırakılır.
5.2'nin teslimi WAV SDK/sürücü/olaylar ve açık CLI'dir.
