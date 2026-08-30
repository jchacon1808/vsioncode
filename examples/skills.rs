//! Qué skills ve vsioncode en esta máquina, y qué le llega al modelo.
//!
//! Es una comprobación contra el disco de verdad: los tests de unidad usan
//! encabezados escritos a mano, y el formato real trae bloques `|`, campos
//! anidados y descripciones de seiscientos caracteres.
//!
//! Y sirve para lo que motivó separar el armario: **enseña de qué carpeta sale
//! cada una**. Las de Claude Code ya no se leen, así que una lista vacía aquí
//! no es un fallo, es un armario sin estrenar — y verlo escrito ahorra
//! buscarle la avería a algo que funciona.
//!
//! ```text
//! cargo run --release --example skills
//! ```

fn main() {
    let cwd = std::env::current_dir().unwrap_or_default();
    let skills = vsioncode::skills::descubrir(&cwd);

    println!("Se miran dos carpetas, y solo esas dos:");
    for d in [
        vsioncode::skills::carpeta_de(&cwd),
        vsioncode::skills::carpeta(),
    ] {
        let estado = if d.is_dir() { "existe" } else { "no existe" };
        println!("  {:<52} {estado}", d.display());
    }

    if skills.is_empty() {
        println!(
            "\nNinguna instalada. No es una avería: el armario de vsioncode es suyo y\n\
             nace vacío. Para traerte una, dentro del programa:\n\n\
             \x20 /skills instalar <usuario/repo>\n\
             \x20 /skills instalar <carpeta del disco>\n\n\
             Sin ninguna, al modelo no le llega catálogo: el apartado no viaja en el\n\
             prompt en vez de anunciar una lista vacía."
        );
        return;
    }

    println!("\nEncontradas {}:\n", skills.len());
    for s in &skills {
        println!("  {:<24} {}", s.nombre, s.ruta.display());
    }

    let cat = vsioncode::skills::catalogo(&skills);
    println!(
        "\n--- catálogo que se le añade al prompt ({} caracteres, en cada turno) ---{cat}",
        cat.chars().count()
    );

    if let Some(primera) = skills.first() {
        match vsioncode::skills::leer(primera) {
            Ok(c) => println!(
                "\n--- «{}» al pedirla: {} caracteres, {} líneas ---",
                primera.nombre,
                c.chars().count(),
                c.lines().count()
            ),
            Err(e) => println!("\nno pude leer «{}»: {e}", primera.nombre),
        }
    }
}
