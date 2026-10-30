// Injects libbackrooms_practice_tool.dll (next to this exe) into the running game.
fn main() {
    let dll = std::env::current_exe().unwrap().with_file_name("libbackrooms_practice_tool.dll");
    let result = if !dll.exists() {
        Err(format!("DLL introuvable : {}", dll.display()))
    } else {
        hudhook::inject::Process::by_name("Inside the Backrooms.exe")
            .map_err(|_| "Jeu introuvable : lance Inside the Backrooms d'abord.".to_string())
            .and_then(|p| p.inject(dll).map_err(|e| format!("Injection ratée : {e:?}")))
    };
    match result {
        Ok(()) => println!("Practice tool injecte. Appuie sur F1 en jeu pour ouvrir le menu."),
        Err(e) => println!("{e}"),
    }
    println!("Appuie sur Entree pour fermer.");
    let _ = std::io::stdin().read_line(&mut String::new());
}
