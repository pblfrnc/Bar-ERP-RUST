//! Grupos automáticos do cardápio digital pelo nome do produto. Tradução de `client/src/utils/menuGroups.ts`
//! (o site `menu/` usa a mesma regra para dividir categorias grandes; a tela "Itens do Cardápio" usa para
//! marcar/desmarcar grupos inteiros e sugerir tirar do cardápio o que é estoque da loja).

use std::sync::LazyLock;

use regex::Regex;

/// Ordem de exibição: comidas, bebidas (cerveja primeiro), depois o que normalmente nem vai ao cardápio.
pub const MENU_GROUP_ORDER: [&str; 20] = [
    "Pizzas",
    "Lanches",
    "Porções e petiscos",
    "Pratos",
    "Sobremesas",
    "Adicionais",
    "Cervejas",
    "Torres",
    "Doses",
    "Drinks prontos e ices",
    "Destilados e licores",
    "Whiskies",
    "Vinhos",
    "Refrigerantes, sucos e águas",
    "Energéticos",
    "Salgadinhos e doces",
    "Mercearia e insumos",
    "Tabacaria",
    "Conveniência e limpeza",
    "Outros",
];

/// Grupos que a tela de itens sugere tirar do cardápio (estoque da loja, não do cardápio).
pub const NOT_FOR_MENU_GROUPS: [&str; 3] = ["Mercearia e insumos", "Conveniência e limpeza", "Tabacaria"];

/// Na ordem de teste do Node: o primeiro que bater leva o item. `\b` é a fronteira ASCII do JavaScript.
const RULES: [(&str, &str); 20] = [
    ("Adicionais", r"^ADICIONAL"),
    ("Tabacaria", r"CIGARRO|^ROTHMANS|LUCKY? STRIKE|ISQUEIRO|ESQUEIRO|MARLBORO|DERBY|DUNHILL|PALHEIRO|\bSEDA\b|NARGUILE|ESSENCIA"),
    (
        "Conveniência e limpeza",
        r"PRESERVATIVO|PAPEL (HIG|TOALHA)|SABAO|DETERG|DESINF|AGUA SANITARIA|SACO PLAST|SACOLA|ISOPOR|COPO DESC|GUARDANAPO|\bPILHA",
    ),
    (
        "Salgadinhos e doces",
        r"^(SALG|AMENDOIM|PIRULITO|PIR\.|CHICLE|CHIC\.|BALAS?\b|CHOC\b|CHOCOLATE|KINDER|FERRERO|BISCOITO|VOTORIA|VITORIA SABOR|HALLS|TIC-TAC|TRIDENT|FINI\b|TUBES|DUCREM|COLORETI|GOMETS|MINTY|STICK|BIG ?BIG|CART X|MINI ENROLADO|PACOCA|PINGO DE LEITE|DOCE\b|MICOS|CHEETOS|DORITOS|RUFFLES|FANDANGOS|LAYS|CEBOLITOS|BACONZITOS|STIKSY|BATATA CRONY|PURURUCA|TORRESMO .*\d+ ?G\b|SEQUILHOS|PETISCO .*\d+ ?G\b|GELADINHO|GELO DE SABOR)",
    ),
    ("Pizzas", r"\bPIZZA"),
    ("Porções e petiscos", r"TABUA|FRIOS|SALAME"),
    (
        "Mercearia e insumos",
        r"CONSERVA|COSERVA|\bVIDRO\b|EM CALDA|SEARA|NISSIN|ODERICH|BORDON|MAIONESE|MOLHO DE (TOMATE|PIMENTA)|MILHO VERDE|GELEIA|BAT(ATA)? PALHA|BATATA PALITO|PRE FRITA|\bCONG\b|\bPCT\b|\d+ ?KG\b|SACHE|HOT POCKET|^AZEITONA|CEBOLINHA|PEPINO|LINGUICA DEFUMADA|^SALSICHA|FLOCOS DE MILHO|CREME DE AVELA|BOLINHO DE CARNE .*\d+ ?G\b|ALMONDEGAS",
    ),
    ("Lanches", r"BURG|HAMBURGUE|^X[- ]|SANDUICHE|LANCHE|HOT ?-?DOG|CACHORRO|\bMISTO\b"),
    (
        "Porções e petiscos",
        r"PORCAO|PORCOES|PETISCO|ISCA|BATATA FRITA|FRITAS?\b|PASSARINHO|CALDO|ACEBOLAD|CARNE DE SOL|MACAXEIRA|PASTEL|PAO DE ALHO|^QUEIJO$|^PRESUNTO|^PALMITO$|EMPANAD",
    ),
    ("Pratos", r"PRATO|EXECUTIVO|REFEICAO|PARMEGIANA|FEIJOADA"),
    ("Torres", r"^TORRE\b"),
    ("Doses", r"^DOSE\b|^DOZE\b"),
    (
        "Drinks prontos e ices",
        r"\bICE\b|BEATS|BALLY|BORABEBE|COROTE|RUSTOFF|ETERNITY|KRISKOF|MANSAO|GIN READY|READY 275|CHUVA DE PRATA|GASEIFICADA|COPAO|CAIPIRINHA|CAIPIROSKA|DRINK",
    ),
    (
        "Cervejas",
        r"^CERV|\bCERVEJA\b|\bSKOL\b|BRAHMA|HEINEKEN|STELLA|CORONA|BUD(W|U)EISER|AMSTEL|^ORIGINAL|KAISER|IMPERIO|CHOPP|LITRAO|LONG NECK",
    ),
    ("Vinhos", r"VINHO|CASILLERO|AURORA|CONDE DE|SANTA HELENA|SANGRIA|ESPUMANTE|CHAMPANHE"),
    (
        "Whiskies",
        r"WH?ISK|WISK|BUCHANAN|OLD PARR|BALL?ANTINES|BALLATINES|BLACK ?& ?WHITE|PASSAPORT|JOHNNIE|LABEL|BLAK STONE|OLDEN|RED GOLD|BLEND\b",
    ),
    (
        "Destilados e licores",
        r"VODKA|VDKA|SMIRNOFF|ABSOLUT|SKYY|\bGIN\b|TANQUERAY|\bRUM\b|TEQUILA|CACHA|CACACHA|YPIOCA|SAGATIBA|VELHO BARREIRO|PRESIDENTE|DREHER|CAMPARI|LICOR|JACK|CUERVO|RAIZ|DOMUS|NOBILIS|MARGARITA|ROCK",
    ),
    ("Energéticos", r"ENERG|RED BULL|MONSTER|ENGOV"),
    (
        "Refrigerantes, sucos e águas",
        r"REFRI|COCA|GUARANA|FANTA|SUKITA|PEPSI|SODA|H2OH|AGUA MINERAL|AGUA COM|AGUA SEM|ACHOC|SUCO|TODDYNHO|\bCHA\b",
    ),
    ("Sobremesas", r"SOBREMESA|PUDIM|SORVETE|\bACAI\b|\bBOLO\b|TORTA|PETIT"),
];

static COMPILED: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    RULES
        .iter()
        .map(|(name, re)| (*name, Regex::new(&re.replace(r"\b", r"(?-u:\b)")).expect("regex dos grupos do cardápio")))
        .collect()
});

/// Sem acento, maiúsculo e com espaços simples (`plainName`).
pub fn plain_name(s: &str) -> String {
    let folded: String = s
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ã' | 'ä' | 'Á' | 'À' | 'Â' | 'Ã' | 'Ä' => 'A',
            'é' | 'è' | 'ê' | 'ë' | 'É' | 'È' | 'Ê' | 'Ë' => 'E',
            'í' | 'ì' | 'î' | 'ï' | 'Í' | 'Ì' | 'Î' | 'Ï' => 'I',
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' | 'Ó' | 'Ò' | 'Ô' | 'Õ' | 'Ö' => 'O',
            'ú' | 'ù' | 'û' | 'ü' | 'Ú' | 'Ù' | 'Û' | 'Ü' => 'U',
            'ç' | 'Ç' => 'C',
            'ñ' | 'Ñ' => 'N',
            other => other,
        })
        .collect::<String>()
        .to_uppercase();
    folded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Grupo do produto pelo nome ("Outros" quando nenhuma regra bate).
pub fn menu_group_of(product_name: &str) -> &'static str {
    let name = plain_name(product_name);
    COMPILED.iter().find(|(_, re)| re.is_match(&name)).map(|(g, _)| *g).unwrap_or("Outros")
}

/// Posição do grupo na ordem de exibição.
pub fn group_rank(group: &str) -> usize {
    MENU_GROUP_ORDER.iter().position(|g| *g == group).unwrap_or(MENU_GROUP_ORDER.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grupos_como_o_site() {
        assert_eq!(menu_group_of("Heineken Long Neck 330ml"), "Cervejas");
        assert_eq!(menu_group_of("SALG AMENDUPA PIZZA"), "Salgadinhos e doces", "conveniência antes de comida");
        assert_eq!(menu_group_of("Panelinha Seara Caldo Verde"), "Mercearia e insumos");
        assert_eq!(menu_group_of("Tábua de frios com azeitona"), "Porções e petiscos");
        assert_eq!(menu_group_of("X-Salada"), "Lanches");
        assert_eq!(menu_group_of("Cigarro Rothmans"), "Tabacaria");
        assert_eq!(menu_group_of("Gin Tônica"), "Destilados e licores");
        assert_eq!(menu_group_of("Ginástica"), "Outros");
        assert_eq!(menu_group_of("Açaí 500"), "Sobremesas");
        assert_eq!(menu_group_of("Pizza Calabresa"), "Pizzas");
        assert_eq!(plain_name("  Água   Tônica "), "AGUA TONICA");
        assert!(group_rank("Pizzas") < group_rank("Cervejas") && group_rank("Cervejas") < group_rank("Outros"));
    }
}
