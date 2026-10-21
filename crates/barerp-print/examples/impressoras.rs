//! Diagnóstico: impressoras instaladas e a padrão do sistema, como a tela "Configurar Impressoras" vê.
//! `cargo run -p barerp-print --example impressoras` (não imprime nada).

fn main() {
    let list = barerp_print::installed_printers();
    println!("Impressoras instaladas ({}):", list.len());
    for p in &list {
        println!("  - {p}");
    }
    match barerp_print::default_printer() {
        Some(p) => println!("Padrão do sistema: {p}"),
        None => println!("Sem impressora padrão no sistema"),
    }
}
