//! Made-up phrases. The two splits use separate word lists, so no string is in both.

use super::{Lang, Split};

type Lists = ([&'static str; 6], [&'static str; 6]);

const ENGLISH: [Lists; 2] = [
    (
        [
            "The silver lantern",
            "A quiet harbor",
            "My older neighbor",
            "The paper dragon",
            "Every winter morning",
            "The last bookshop",
        ],
        [
            "waits by the river",
            "keeps a small secret",
            "returns before dawn",
            "forgot the blue key",
            "sings to the clouds",
            "paints the old bridge",
        ],
    ),
    (
        [
            "That crooked tower",
            "Her borrowed umbrella",
            "The midnight baker",
            "Our distant garden",
            "A sleepy conductor",
            "This broken compass",
        ],
        [
            "counts every falling star",
            "hides under the stairs",
            "opens only on Sundays",
            "follows the red train",
            "remembers your name",
            "melts into the fog",
        ],
    ),
];

const SPANISH: [Lists; 2] = [
    (
        [
            "El pequeño dragón",
            "La última estación",
            "Mi vecina mayor",
            "¿Dónde está el niño?",
            "La canción azul",
            "¡Qué difícil día!",
        ],
        [
            "busca la sartén",
            "cruzó el océano",
            "pidió más café",
            "pintó un corazón",
            "escuchó la lluvia",
            "guardó un secreto",
        ],
    ),
    (
        [
            "Una montaña lejana",
            "El señor Muñoz",
            "La niña pelirroja",
            "¿Quién abrió el baúl?",
            "Nuestro último tren",
            "¡Allá va la cigüeña!",
        ],
        [
            "volverá mañana",
            "soñó con piñatas",
            "perdió el avión",
            "cambió de opinión",
            "miró hacia atrás",
            "comió melocotón",
        ],
    ),
];

const ROMAJI: [Lists; 2] = [
    (
        [
            "Hoshizora no",
            "Kaze no naka",
            "Ame agari no",
            "Yuki furu hi",
            "Tsukiyo no",
            "Haru matsu",
        ],
        [
            "yakusoku",
            "tegami",
            "sanpomichi",
            "kissaten",
            "onee-san",
            "monogatari",
        ],
    ),
    (
        [
            "Umibe de",
            "Houkago kara",
            "Akane iro",
            "Mayonaka made",
            "Natsu yasumi",
            "Sakura saku",
        ],
        [
            "himitsu",
            "kaerimichi",
            "toshokan",
            "koibito",
            "rensai kiroku",
            "amayadori",
        ],
    ),
];

const JAPANESE: [Lists; 2] = [
    (
        [
            "星空の下で",
            "静かな図書館に",
            "雨上がりの道を",
            "小さな喫茶店の",
            "夕暮れの駅前で",
            "春を待つ町は",
        ],
        [
            "猫がねむる",
            "手紙を書いた",
            "約束を守る",
            "コーヒーを飲む",
            "新しい本を読む",
            "風鈴がなる",
        ],
    ),
    (
        [
            "海辺の灯台から",
            "放課後の教室で",
            "古い神社の裏に",
            "夜明けの空へ",
            "雪の降る村では",
            "隣の席の先輩が",
        ],
        [
            "鳥が飛び立つ",
            "秘密を話した",
            "花火を見上げる",
            "ギターを弾く",
            "お弁当を食べる",
            "小説を借りる",
        ],
    ),
];

const CHINESE: [Lists; 2] = [
    (
        [
            "蓝色的月亮",
            "安静的小城",
            "清晨的花园",
            "远方的朋友",
            "旧书店老板",
            "山间的小路",
        ],
        [
            "正在慢慢醒来",
            "写了一封长信",
            "等待春天到来",
            "喜欢喝热茶",
            "看着窗外细雨",
            "收藏许多地图",
        ],
    ),
    (
        [
            "会唱歌的风筝",
            "河边的灯笼",
            "沉默的钟表匠",
            "邻居家黑猫",
            "深夜的面包房",
            "雪后的操场",
        ],
        [
            "突然飞上天空",
            "照亮回家的路",
            "忘记了时间",
            "偷偷藏起钥匙",
            "飘出温暖香气",
            "留下一串脚印",
        ],
    ),
];

const KOREAN: [Lists; 2] = [
    (
        [
            "푸른 달빛 아래",
            "조용한 도서관에서",
            "비 갠 골목길을",
            "작은 찻집 주인은",
            "저녁 무렵 기차역에",
            "봄을 기다리는 마을",
        ],
        [
            "고양이가 잠든다",
            "편지를 썼다",
            "약속을 지킨다",
            "천천히 걸어간다",
            "새 책을 읽는다",
            "종소리가 울린다",
        ],
    ),
    (
        [
            "바닷가 등대에서",
            "방과 후 교실에",
            "오래된 사당 뒤편",
            "새벽 하늘 너머로",
            "눈 내리는 산골에",
            "옆자리 선배님이",
        ],
        [
            "새들이 날아오른다",
            "비밀을 털어놓았다",
            "불꽃놀이를 구경해",
            "우산을 깜빡했어요",
            "도시락을 나눠 먹는다",
            "소설책을 빌려 간다",
        ],
    ),
];

/// 36 two-part phrases followed by the 12 parts on their own.
fn combine(lists: &Lists, separator: &str) -> Vec<String> {
    let (first, second) = lists;
    let mut out: Vec<String> = (0..36)
        .map(|i| {
            // Walks every pair while changing both halves from one phrase to the next.
            let (a, b) = (i % 6, (i + i / 6) % 6);
            format!("{}{separator}{}", first[a], second[b])
        })
        .collect();
    out.extend(first.iter().chain(second).map(|s| s.to_string()));
    out
}

/// Distinct phrases: 48 each in Japanese, Chinese and Korean; 144 in Latin script, cycling
/// through English, accented Spanish and romanised Japanese.
pub(super) fn phrases(split: Split, lang: Lang) -> Vec<String> {
    let i = split as usize;
    match lang {
        Lang::Japanese => combine(&JAPANESE[i], ""),
        Lang::Chinese => combine(&CHINESE[i], ""),
        Lang::Korean => combine(&KOREAN[i], " "),
        Lang::Latin => {
            let sets = [
                combine(&ENGLISH[i], " "),
                combine(&SPANISH[i], " "),
                combine(&ROMAJI[i], " "),
            ];
            (0..sets[0].len())
                .flat_map(|n| sets.iter().map(move |set| set[n].clone()))
                .collect()
        }
    }
}

type Titles = ([&'static str; 8], [&'static str; 8]);

const LATIN_TITLES: [Titles; 2] = [
    (
        [
            "MI VECINA",
            "EL JARDÍN",
            "LA CHICA GAMER",
            "UN GATO",
            "HOSHIZORA",
            "THE QUIET",
            "KAZE NO UTA",
            "SEÑORITA NUBE",
        ],
        [
            "DE NIEBLA AZUL",
            "QUE NO DUERME",
            "DEL PISO TRES",
            "AND THE LIGHTHOUSE",
            "YAKUSOKU-CHAN",
            "COLLECTORS CLUB",
            "TOKIDOKI SMILES",
            "TRAS EL MOSTRADOR",
        ],
    ),
    (
        [
            "MI JEFA",
            "EL FANTASMA",
            "LA PRINCESA",
            "UN ROBOT TÍMIDO",
            "UMIBE NO KOI",
            "THE LAZY WITCH",
            "AKANE-SAN",
            "DOÑA TORMENTA",
        ],
        [
            "DE LOS LUNES",
            "VENDE FLORES",
            "DEL ÚLTIMO VAGÓN",
            "AND THE PAPER CROWN",
            "WA TSUTAETAI",
            "BAKERY DIARIES",
            "SOMETIMES CRIES",
            "BAJO LA ESCALERA",
        ],
    ),
];

const JAPANESE_TITLES: [Lists; 2] = [
    (
        [
            "星降る町の",
            "となりの席の",
            "眠れない夜の",
            "秘密の花園と",
            "白い帆船の",
            "図書委員の",
        ],
        [
            "郵便屋さん",
            "小さな約束",
            "ねこ日和",
            "魔法使い見習い",
            "あまい寄り道",
            "午後三時",
        ],
    ),
    (
        [
            "雨の日だけの",
            "生徒会長と",
            "海が見える",
            "忘れられた",
            "双子の妹は",
            "夏祭りの",
        ],
        [
            "純喫茶",
            "風鈴姫",
            "観測日記",
            "おとぎ話",
            "天才画家",
            "金魚すくい",
        ],
    ),
];

const KOREAN_TITLES: [Lists; 2] = [
    (
        [
            "달빛 속의",
            "옆집 누나는",
            "비밀 정원의",
            "잠 못 드는",
            "하얀 돛단배",
            "도서부원의",
        ],
        [
            "우체부 소녀",
            "작은 맹세",
            "고양이 일기",
            "견습 마법사",
            "달콤한 방과후",
            "오후 세 시",
        ],
    ),
    (
        [
            "비 오는 날의",
            "학생회장과",
            "바다가 보이는",
            "잊혀진 왕국",
            "쌍둥이 동생은",
            "여름 축제의",
        ],
        [
            "찻집 이야기",
            "바람 공주",
            "관찰 노트",
            "옛날 동화",
            "천재 화가",
            "금붕어 놀이",
        ],
    ),
];

/// The caption of the `n`-th made-up work in `lang`, as one or two lines.
pub(super) fn title(split: Split, lang: Lang, n: usize) -> Vec<String> {
    let i = split as usize;
    let pair = |a: &str, b: &str| vec![a.to_string(), b.to_string()];
    match lang {
        Lang::Japanese => {
            let (first, second) = &JAPANESE_TITLES[i];
            pair(first[n % 6], second[(n + n / 6) % 6])
        }
        Lang::Korean => {
            let (first, second) = &KOREAN_TITLES[i];
            pair(first[n % 6], second[(n + n / 6) % 6])
        }
        _ => {
            let (first, second) = &LATIN_TITLES[i];
            let (a, b) = (first[n % 8], second[(n + n / 8) % 8]);
            // Every fourth work has a one-line caption.
            if n % 4 == 3 {
                vec![b.to_string()]
            } else {
                pair(a, b)
            }
        }
    }
}
