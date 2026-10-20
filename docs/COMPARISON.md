# 7.4 — Namzu-RS / Jcode deney protokolü

Durum: **hazırlık tamam, canlı koşular giriş bekliyor**. Bu oturumda
`namzu-rs auth status` sonucu `authenticated: false`; önceki pilotun canlı
kimlik bilgileri mevcut profile taşınmış görünmüyor. Gerçek model sonucu
üretilmedi. 7.4 bitmiş sayılmaz.

## Hazırlanan deney

- Namzu-RS: 7.3 kaynak commit'i `99fd14409143de4d90d6d73e5b08101bc86fb746`.
- Jcode: resmî **v0.98.1 (1c7426eb1)** Linux x86_64 release'i yeniden kuruldu.
  Arşiv SHA-256'sı GitHub release digest'iyle doğrulandı:
  `449a8ed50aa9ac75e6afe9b8e916814172ff7cc63913c7ae1f9abd5b1bc5323c`.
  Wrapper ve asıl `.bin` dosyası ignored artifacts içinde, kaynak deposunda değil.
- Planlanan model **gpt-6-luna**, effort **low**. Önceki pilotta kullanılan
  seçimdir; bu oturumdaki erişimi girişten sonra yeniden doğrulanmalı.
- Beş görev: slug, intervals, toposort, ledger, refactor. İki tekrar, her
  görev/ajan/tekrar için ayrı temiz workspace: **20 seri koşu**.
- Ajan başına **300 saniye**; bağımsız checker ayrıca en fazla 30 saniye.
- İlk tekrar Namzu → Jcode; ikinci tekrar Jcode → Namzu. Aynı prompt,
  başlangıç dosyaları ve host oraklları, ortak model ve effort seçimi.
- Her iki tarafta host dosya okuma/yazma/shell yetkisi. Namzu araçları
  `read_file,list_files,search_files,write_file,apply_patch,shell` ile sınırlandırılır;
  Jcode `bash,read,write,edit,apply_patch,glob,grep`. Bunlar eşit yetki düzeyi
  beyanıdır; araç uygulamaları ve sistem prompt'ları birebir aynı değildir.
- Jcode update/selfdev kapalı, ayrı socket; telemetri kapalı. Yeni bir özel
  Jcode profiline CLI köprüsüyle erişim/identity tokenı aktarılır; refresh grant
  kopyalanmaz. Auth içeriği veya gerçek anahtarlar suite'e/rapora konmaz.

Hazırlanan yerel suite: ignored `artifacts/comparison-suite.json`.
`bench validate` geçti. Henüz canlı model çağrısı başlatılmadı. Kurulum ve giriş
durumu, geçmişteki üç görevlik canlı kabulün tekrarlandığı anlamına gelmez.

## Girişten sonra devam

1. `namzu-rs auth status` ve `namzu-rs models` ile canlı girişi/modeli doğrula.
2. `auth link-jcode --directory YENI_OZEL_PROFIL` ile Jcode'u bağla; yalnızca bu
   profilin config.toml'ında `[provider] openai_reasoning_effort = "low"` seç.
3. Run ortamında `JCODE_HOME` yeni profil; `JCODE_OPENAI_REASONING_EFFORT=low`,
   `JCODE_OPENAI_TRANSPORT=https`, `JCODE_NO_TELEMETRY=1`. Bu ayarlar zaten
   suite'in env-name allowlist'ine dahil; değerler suite'e kaydedilmez.
4. İki ürünün modeli gerçekten seçebildiğini doğrula; fallback kullanma. Sonra
   hazırlanmış suite'i `bench run` ile sırayla çalıştır. Hatalı görevleri sonuçtan
   silme veya başarılı olana kadar sessiz tekrar yapma; her yeni deney ayrı rapor.
5. Tamamlanmış `report.json` dosyasını aşağıdaki araca ver; sayısal raporu ve
   karşılaştırma koşullarını inceleyip sonuçlarla bu belgeyi güncelle.

## Rapor doğrulama ve özet

```sh
python3 -m unittest discover -s scripts -p test_compare_report.py -v
python3 scripts/compare_report.py artifacts/KOSU/report.json \
  --out artifacts/comparison-numeric.json
```

Araç model çağırmaz. Yalnızca tamamlanmış v2 **live**, seri, 300 saniyelik,
tam beş görev/iki ajan matrisi kabul eder. Tekrar sayısı çift ve en az ikidir;
ters ajan sırası, eşleşen prompt/input/oracle hash'leri, sabit program hash'leri
ve ortak izin beyanı zorunludur. Yanlış çözüm ve orakl hatası ayrı kalır.
Normal exit 0 ve doğru orakl olmadan `passed` kabul edilmez.

Çıktı; görev kimliği, tekrar/sıra, status, hash'ler, süre/RSS ve bilinen sayısal
çağrı sayılarının allowlist'idir. Raw ajan/checker çıktısı, yerel dosya yolları,
session veya auth kimliği kopyalanmaz. Çıktı **yeni 0600 dosya** olarak yazılır;
mevcut dosya ezilmez. Model kimliği ve sayısal sonuçlar yayımlanmadan incelenir.

Her ajan için toplam durumlar, tüm koşuların toplam/medyan süresi, yalnızca
geçenlerin medyan süresi ve ölçülen maksimum/medyan peak RSS üretilir. Eşlenmiş
hız oranı yalnızca **iki ajanın da geçtiği** görev/tekrar çiftlerinden hesaplanır:
Jcode süresi / Namzu süresi. Oran >1 o çiftlerde Namzu'nun daha kısa sürdüğünü
gösterir; başarısız bir hızlı çıkış hız kazancı sayılmaz.

Çağrı sayıları ajanın bildirdiği verilerdir. Her sayaç için bilinen koşu sayısı
ve bunların toplamı raporlanır; hiç bilinmiyorsa toplam **null** kalır. Kısmi
kapsama bütün deneyin toplam çağrısıymış gibi sunulmaz. Süreler checker süresini
içermez; RSS `wait4.ru_maxrss` ölçümüdür, eşzamanlı bütün süreçlerin toplam RAM/PSS'si
değildir. Jcode wrapper hash'i `.bin`/bağımlılıkların hash'i yerine geçmez.

Beş offline Python test grubu; eksik/fixture rapor reddi, dengesiz sıra, değişen
program/görev hash'i, redaksiyon, bilinmeyen sayaç ve yalnızca ortak başarıdaki
hız hesabını doğruladı. Sentetik test verileri gerçek model sonucu değildir.
Küçük örneklemden istatistiksel üstünlük veya programlama dilinin etkisi çıkarılmaz.
