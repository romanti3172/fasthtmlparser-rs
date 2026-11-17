Windows 10, AMD Ryzen 7 3800X, rustc 1.100.0, `cargo run --release` in bench/ (2026-11-17).

| task | library | time | throughput |
|---|---|---:|---:|
| parse quotes 11 KB | fasthtmlparser | 16.6 µs | 682 MB/s |
| parse quotes 11 KB | scraper (html5ever) | 366.7 µs | 31 MB/s |
| parse quotes 11 KB | tl | 38.2 µs | 297 MB/s |
| parse quotes 11 KB | skyscraper | 458.0 µs | 25 MB/s |
| parse laptops 38 KB | fasthtmlparser | 52.3 µs | 739 MB/s |
| parse laptops 38 KB | scraper (html5ever) | 1130.8 µs | 34 MB/s |
| parse laptops 38 KB | tl | 153.4 µs | 252 MB/s |
| parse laptops 38 KB | skyscraper | 1223.9 µs | 32 MB/s |
| parse books 51 KB | fasthtmlparser | 58.4 µs | 916 MB/s |
| parse books 51 KB | scraper (html5ever) | 1546.1 µs | 35 MB/s |
| parse books 51 KB | tl | 111.1 µs | 482 MB/s |
| parse books 51 KB | skyscraper | 1368.2 µs | 39 MB/s |
| parse books x10 510 KB | fasthtmlparser | 578.3 µs | 926 MB/s |
| parse books x10 510 KB | scraper (html5ever) | 15233.0 µs | 35 MB/s |
| parse books x10 510 KB | tl | 1976.2 µs | 271 MB/s |
| parse books x10 510 KB | skyscraper | 14789.0 µs | 36 MB/s |
| parse books, no </li> </p> | fasthtmlparser | 60.1 µs | 880 MB/s |
| parse books, no </li> </p> | scraper (html5ever) | 1584.8 µs | 33 MB/s |
| parse books, no </li> </p> | tl | 110.1 µs | 480 MB/s |
| parse books, no </li> </p> | skyscraper | 1263.1 µs | 42 MB/s |
| scrape 20 books | fasthtmlparser find | 74.6 µs |  |
| scrape 20 books | fasthtmlparser CSS | 82.7 µs |  |
| scrape 20 books | fasthtmlparser XPath | 194.5 µs |  |
| scrape 20 books | scraper CSS | 1619.2 µs |  |
| scrape 20 books | tl CSS | 171.8 µs |  |
| scrape 20 books | skyscraper XPath | 2034.7 µs |  |
| scrape 10 quotes | fasthtmlparser find | 30.1 µs |  |
| scrape 10 quotes | fasthtmlparser CSS | 30.8 µs |  |
| scrape 10 quotes | scraper CSS | 405.0 µs |  |
