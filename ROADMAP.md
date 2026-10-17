# Yeniden kurulum durumu

2026-10-17: Eski kaynaklar mevcut ortamda bulunamadı. Aşağıdaki tüm modüller
yeniden doğrulanana kadar eksik sayılır. Önceki sohbetin tamamlandı kayıtları
bu yeni depoda tamamlanmış uygulama anlamına gelmez.

## İlk aşama

- [x] Rust workspace ve SDK/ajan çekirdeğinin ilk sürümü
- [x] Deterministik sağlayıcı ile araç çağrısı döngüsü
- [x] Sınırlı dosya araçları, terminal komutu ve JSON çıktı sözleşmesi
- [x] Süre (varsayılan 300 saniye), adım ve araç çağrısı bütçeleri
- [x] 34 test; Clippy (uyarılar hata); format; release derlemesi ve release demo
- [ ] Uzak, özel Git deposuna doğrulanmış yedek

İlk sürüm OpenAI uyumlu Chat Completions adaptörü içerir. Yerel HTTP fixture
ile CLI → sağlayıcı → dosya aracı → sağlayıcı sonucu zinciri sınandı.
Codex abonelik girişi ve eski benchmark script uyumu henüz yoktur.

## Doğrulanan ilk aşama — 2026-10-17

- CLI: 3 süreç testi + 2 gerçek loopback HTTP testi.
- Core: 1 deterministik deadline testi + 13 ajan döngüsü/bütçe/iptal testi.
- Sağlayıcı: 6 protokol/URL/anahtar redaksiyonu testi.
- Dosyalar: 9 yetenek sınırı/symlink/readonly/boyut/FIFO testi.
- `cargo test --workspace --locked`: 34 geçti, 0 başarısız, 0 atlandı.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: geçti.
- `cargo fmt --all -- --check`: geçti.
- `cargo build --release --workspace --locked`: geçti.
- Release `demo --json`: çıktı 12, 3 scripted sağlayıcı adımı, 2 dosya çağrısı.
- Harici model çağrısı: 0. Gerçek sağlayıcı hesabı/kota doğrulanmadı.
- Checkpoint bundle'ından geçici dizine clone sonrası Git kaynak ağacı
  eşitliği doğrulandı. Dış GitHub yedeği henüz yok; hedef depo bekleniyor.

## Önceki kapsam

1. Etkileşimli terminal / TUI.
2. Alt ajanlar / Pals: paralel ekipler, otomatik mesaj işleme, Pal bilgisayarları.
3. MCP: 3.1 protokol, 3.2 stdio, 3.3 HTTP, 3.4 yönetim/izinler,
   3.5 keşif/sağlayıcılar, 3.6 SDK/CLI/Pal entegrasyonu,
   3.7 OAuth/kimlik bilgileri, 3.8 toparlanma ve birlikte çalışabilirlik.
   Bu alt başlıklar sohbetten yeniden çıkarılmış uygulama planıdır.
4. Tarayıcı / bilgisayar kontrolü.
5. Dosya, ses, LSP ve sandbox paketleri.
6. Model sağlayıcıları; Codex aboneliği entegrasyonu ve benchmark uyumu.
7. Telemetri, AG-UI ve değerlendirme araçları.
8. Masaüstü uygulaması.

## Kayıp kaynağı tanıma bilgisi

Son eski arşiv: namzu-rs-source.tar.gz; 138 kaynak dosyası; 394421 bayt.
Sohbet kaydındaki SHA-256:
`b994799c95736271078b8491efb37bb60a74cb2b7f087c95ab21a2eec69917ba`.
Eski rapor 297 test ve 9 Docker kontrolü bildiriyordu; bu sonuçlar yeni
uygulamanın doğrulaması olarak kullanılamaz. 3.7 eski projede başlamamıştı.
