# 2.5 — Pal bilgisayarları

`namzu-computer`, Linux üzerinde native Rust guest köprüsü kullanan, sahip olunan
Docker container yaşam döngüsüdür. Çekirdek SDK Docker'a bağımlı değildir.
`Registry::computers(owner, pal_id, Docker::local(...))` Pal sahipliğini doğrular
ve bilgisayar kaydını özel Pal metadata dizininde tutar. Registry'nin host çıktı
klasörü container'a bağlanmaz; guest `/workspace` ayrı bir named volume'dur.

## Kullanım ve görüntü

```sh
python3 scripts/build_pal_image.py
cargo run -p namzu-computer --example smoke --locked -- sha256:IMAGE_ID
```

İlk komut native `namzu-guest` binary'sini derler, `containers/pal/Dockerfile`
ile image oluşturur; sabit image ID'si ve binary hash'i özel olmayan
`artifacts/pal-image.json` dosyasına yazılır. Bu dosya build çıktısıdır, Git'e
girmez. Runtime image çekmez/derlemez: yerel `sha256:<64 hex>` ID'si ve guest
protocol 1 ister. Tag, eksik image, hatalı protokol veya Docker bağlantı sorunu
host shell/dosya araçlarına fallback oluşturmaz.

Image Debian Trixie digest'ine sabitlenmiştir; build yalnızca guest binary'sini
kopyalar ve yerel dizin/izin hazırlığı yapar, ağlı RUN adımı yoktur. Guest binary
Linux/daemon mimarisiyle ve image libc'siyle uyumlu derlenmelidir. Cloud'daki
native glibc 2.41 binary/image çifti gerçek container'da doğrulandı. Registry
kimlik bilgileri, model tokenları, proje klasörü veya session dosyaları build
context'ine kopyalanmaz. Docker'ın yönetilen proxy/CA/registry yapılandırması
korunur; runtime yalnızca yerel Unix socket'i kullanır, uzak Docker context/TLS
seçicileri devralınmaz. Guest'in ağı bu aşamada kapalıdır.

## Sınırlar

- Varsayılan RAM 256 MiB, CPU 1 çekirdek, PID 64. Host 32 MiB–2 GiB,
  0.1–4 CPU, 16–256 PID arasında açık seçim yapabilir. Memory-swap aynı byte
  tavanına ayarlanır; ayrıca 16 MiB `/tmp` tmpfs ve nofile/core ulimit'i vardır.
- Root filesystem read-only, kullanıcı `65532:65532`, capabilities `ALL` drop,
  `no-new-privileges`, özel IPC/cgroup namespace, `network=none`, restart policy
  `no`. Host bind mount, host network, Docker socket, cihaz ve privileged mod yok.
  Her etki öncesi labels, image, mount ve confinement ayarları yeniden denetlenir.
- `/workspace` yalnızca o bilgisayarın named volume'udur. Farklı Pals'ın volume
  ve container adları/scope'ları ayrıdır. Shell guest root filesystem'i okuyabilir;
  host filesystem'ini veya diğer Pal volume'larını bağlamaz.
- RAM/CPU/PID tavanlarının kernel'de uygulandığı gerçek cgroup dosyalarından
  doğrulandı. **Named volume için hard disk kotası yoktur.** 64 KiB dosya aktarım
  sınırı shell'in diske yazmasını sınırlamaz; filesystem quota/network politikası
  5.4'te genişletilecektir. Docker ortak host kernel'ini kullanır, VM değildir.

## Yaşam döngüsü ve sahiplik

`ComputerManager::acquire(spec, context)` süreçler arası kilit alır ve ilk daemon
etkisinden önce `Creating` kaydı yazar. Rastgele scope + artan generation,
container ve volume'un owner/Pal/generation label'larıyla bağlanır. Ham snapshot
bir kontrol yetkisi değildir; `Computer` lease'i clone edilemez. Aynı metadata
kökünde ikinci sahip kilitle reddedilir.

`stop` tüm container süreçlerini öldürür ve durduğunu doğrular; volume kalır.
`start/restart` yalnızca idle guest köprüsünü başlatır, eski komutu tekrar
göndermez. Dosyalar named volume sayesinde korunur. `detach`, bilgisayarı durdurup
kaydını/volume'u saklar; `resume` bunu açıkça devralır. Resume eski pending etki
bulursa devam etmeden önce eski container'ı durdurur. Eksik/yanlış lifecycle
durumunu otomatik yeni container'a dönüştürmez.

`close` sadece doğru etiketli container'ı immutable ID'siyle ve doğru etiketli
volume'u adıyla siler; sonrasında ikisinin de yokluğunu doğrular. Herhangi birini
silmeden önce her iki kaynağın sahipliği denetlenir. Başkasının etiketini taşıyan
kaynağa dokunulmaz; toplu/wildcard silme yapılmaz. `recover_cleanup` yarıda kalan
oluşturma/kapatmayı aynı kuralla tamamlar. Generation yeniden kullanılmaz.
Daemon ve ona erişebilen host yöneticileri güvenilen kontrol düzlemidir; Docker
yöneticisine karşı label/volume adı üzerinden atomik güvenlik sınırı iddiası yoktur.

## Dosya ve komut API'si

- `upload/download`: UTF-8 göreli path üzerinden en fazla 64 KiB **ikili içerik**.
  Base64 JSON stdin taşıması host shell'den geçmez. Guest dosyaları `cap-std`
  dizin yeteneğiyle açar; mutlak/`..` yollar, kök dışı symlink, FIFO/regular-file
  ve byte sınırları denetlenir. Yazma, guest içinde atomik rename + fsync yapar;
  yeni dosya modu 0600'dür. Yaprak symlink'e yazmak linki değiştirir, hedefini değil.
- `list`: en fazla 512 girdi; wire sınırı da 512 KiB'dir. Eksik/kesilmiş yanıt
  başarı olarak kabul edilmez.
- `shell`: guest `/bin/sh`, göreli ve workspace içinde canonical cwd, en fazla
  300 saniye ve parent context kalan süresi; 64 KiB birleşik ham stdout/stderr.
  Metin kayıplı UTF-8 dönüşümünde genişleyebilir; guest JSON yanıtı ayrıca sınırlıdır.
  stdin/stdout aynı anda bounded/nonblocking yürütülür; pipe deadlock'u yoktur.
- Normal bitişte süreç grubu ve subreaper'ın sahip olduğu, `setsid`/double-fork ile
  ayrılan çocuklar temizlenir. Zaman/çıktı sınırı, iptal, transport/protokol hatası
  guest çalışmasını yalnızca Docker CLI'sini keserek bırakmaz; tüm container'ı
  durdurur. Normal başarısız exit code araç sonucu olabilir; otomatik retry yoktur.

`GuestTools` bu API'yi `read_file/list_files/write_file/shell` olarak SDK'ye verir.
Varsayılan yalnızca okuma; yazma/shell açık host izni ister. Allow/deny her iki
tanım/dispatch sınırında geçerlidir. Pal'ın pinned araç profili ve üst host onayı
ayrıca `PalRun` katmanında uygulanır. Binary dosyalar host transfer API'siyle
taşınır; modelin `read_file` aracı UTF-8 bekler. Model anahtarları Docker CLI
çocuk ortamına aktarılmaz. Yönetilen Docker proxy varsayılanları korunur.

## Çökme, iptal ve veri koruma

Her guest etkisinden önce özel kayıtta `pending` vardır. Taşıma kesilmesi,
bilinmeyen yanıt veya başarısız shell/yazma etkisi pending bırakır; bilgisayar
durdurulur. Context iptal edilmiş olsa da temizlik için ayrı, sınırlı 20 saniyelik
context kullanılır. Uzak durdurma başarısızsa hata ve kalıcı `CleanupRequired`
kaydı vardır; temizlik başarılıymış gibi gösterilmez.

Belirsiz kayıtta açık start sonrası yalnızca probe/okuma/listeleme yapılabilir;
yeni yazma/shell, host `acknowledge_uncertain` çağırana kadar reddedilir. Bu çağrı
eski komutu tekrarlamaz veya etkisini geri almaz. `close/recover_cleanup` çalışma
alanını da sildiğinden önce gerekli dosyalar dışarı alınmalıdır.

Sağlıklı lease drop'u kaynakları kapatıp siler. Pending veya tamamlanmamış
lifecycle varsa drop önce durdurmayı dener ve inceleme için volume/kaydı korur;
bu durumda host açık temizlik yapmalıdır. Drop hata döndüremez; başarısızlık kaydı
snapshot'tan okunur. SIGKILL/host/daemon çökmesi destructor çalıştırmaz; sonraki
resume/cleanup kalıcı sahiplik kaydından toparlar. Disk/daemon yokken mutlak
temizlik veya exactly-once uzak etki garantisi verilmez.

## Kabul

174 Rust testi (Docker/hesap gerektirmez), workspace format/Clippy, release build.
Gerçek Docker kabulünde 15 kontrol: ayrı Pal dosyaları; 64 KiB binary; host state,
ağ/capability/anahtar aktarımı; kernel RAM/CPU/PID; read-only root; symlink/FIFO/`..`;
kaçan çocuk temizliği; dosyaları koruyan restart; timeout; canlı iptal; çıktı
sınırı; gerçek SDK guest araç döngüsü; container/volume temizliği; gerçek host
client SIGKILL sonrası pending etkiyi tekrar etmeden inceleme. Canlı model çağrısı 0.

Normal `cargo test` Docker başlatmaz. Gerçek kabul yukarıdaki smoke örneğiyle açıkça
çalışır. Genel süreç supervisor'ına stdin eklendiği için mevcut 25 Linux TUI PTY
senaryosu da yeniden doğrulanır. Pal/CLI/TUI'nin normal kullanıcı akışlarını bu
bilgisayarlara bağlama ve uçtan uca kabul **2.6**'da tamamlandı;
[PALS-CLI.md](PALS-CLI.md) ve [kabul](PALS-ACCEPTANCE.md). Görsel masaüstü ve
tarayıcı eylemleri **4. aşama** kapsamındadır.


## 2.6 host yaşam döngüsü

`GuestTools::into_computer` lease'i hosta geri verir. `release` sağlıklı idle
bilgisayarı çalışır/stopped haliyle açıkça korur; pending lifecycle bırakamaz.
`preserve_workspace_on_drop`, kalıcı CLI bilgisayarında erken dönüş/panic
sırasında volume silinmesini önler; Drop remote işi yine bağımsız bounded cleanup
ile durdurur. `detach` stop+retain, `close` owned cleanup olarak kalır. Normal
SDK'nin varsayılan sağlıklı Drop temizliği değişmez. CLI görev/aktarımlar sonunda
kalıcı volume durdurularak saklanır; geçici sağlıklı bilgisayar açıkça silinir.
Pending bilgisayarda model yazması engellenir, host inspection/ack gerekir.
