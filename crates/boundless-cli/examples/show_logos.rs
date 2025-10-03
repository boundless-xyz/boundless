use colored::Colorize;

fn main() {
    println!("\n{}\n", "=== OPTION 1: Cyan Gradient with Border ===".bold());
    println!("{}", "╔═══════════════════════════════════╗".cyan().bold());
    println!("{}     .MMMMMMMMMM,                  {}", "║".cyan().bold(), "║".cyan().bold());
    println!("{}  {}              {}", "║".cyan().bold(), ",MMMMO     cMMMMMc".bright_cyan().bold(), "║".cyan().bold());
    println!("{} {}               {}", "║".cyan().bold(), "WMMMMx       .MMMMMM".cyan().bold(), "║".cyan().bold());
    println!("{}{} {}", "║".cyan().bold(), "0MMMMM'        ;MMMMM0".bright_cyan().bold(), "║".cyan().bold());
    println!("{}{}{}", "║".cyan().bold(), ":MMMMM.           :MMM".cyan().bold(), "║".cyan().bold());
    println!("{}      {} {}", "║".cyan().bold(), "0MMMMM;        .MMMMMK".bright_cyan().bold(), "║".cyan().bold());
    println!("{}       {}        {}", "║".cyan().bold(), "MMMMMM.       dMMMMM".cyan().bold(), "║".cyan().bold());
    println!("{}        {}       {}", "║".cyan().bold(), "lMMMMM;     dMMMMc".bright_cyan().bold(), "║".cyan().bold());
    println!("{}           {}  {}", "║".cyan().bold(), "lMMMMMMMMMM:".cyan().bold(), "║".cyan().bold());
    println!("{}               {}               {}", "║".cyan().bold(), "lMM;".bright_cyan().bold(), "║".cyan().bold());
    println!("{}", "╚═══════════════════════════════════╝".cyan().bold());

    println!("\n{}\n", "=== OPTION 2: Purple & Blue Neon ===".bold());
    println!("{}", "     .MMMMMMMMMM,".bright_magenta().bold());
    println!("  {}", ",MMMMO     cMMMMMc".magenta().bold());
    println!(" {}", "WMMMMx       .MMMMMM".bright_blue().bold());
    println!("{}", "0MMMMM'        ;MMMMM0".blue().bold());
    println!("{}", ":MMMMM.           :MMM".bright_magenta().bold());
    println!("      {}", "0MMMMM;        .MMMMMK".magenta().bold());
    println!("       {}", "MMMMMM.       dMMMMM".bright_blue().bold());
    println!("        {}", "lMMMMM;     dMMMMc".blue().bold());
    println!("           {}", "lMMMMMMMMMM:".bright_magenta().bold());
    println!("               {}", "lMM;".magenta().bold());

    println!("\n{}\n", "=== OPTION 3: Green Matrix Style ===".bold());
    println!("{}", "     .MMMMMMMMMM,".black().on_bright_green().bold());
    println!("  {}", ",MMMMO     cMMMMMc".bright_green().bold());
    println!(" {}", "WMMMMx       .MMMMMM".green().bold());
    println!("{}", "0MMMMM'        ;MMMMM0".bright_green().bold());
    println!("{}", ":MMMMM.           :MMM".green().bold());
    println!("      {}", "0MMMMM;        .MMMMMK".bright_green().bold());
    println!("       {}", "MMMMMM.       dMMMMM".green().bold());
    println!("        {}", "lMMMMM;     dMMMMc".bright_green().bold());
    println!("           {}", "lMMMMMMMMMM:".green().bold());
    println!("               {}", "lMM;".bright_green().bold());

    println!("\n{}\n", "=== OPTION 4: Gold Elegance ===".bold());
    println!("{}", "    ✧･ﾟ: *✧･ﾟ:* BOUNDLESS *:･ﾟ✧*:･ﾟ✧".yellow().bold());
    println!("{}", "     .MMMMMMMMMM,".bright_yellow().bold());
    println!("  {}", ",MMMMO     cMMMMMc".yellow().bold());
    println!(" {}", "WMMMMx       .MMMMMM".bright_yellow().bold());
    println!("{}", "0MMMMM'        ;MMMMM0".yellow().bold());
    println!("{}", ":MMMMM.           :MMM".bright_yellow().bold());
    println!("      {}", "0MMMMM;        .MMMMMK".yellow().bold());
    println!("       {}", "MMMMMM.       dMMMMM".bright_yellow().bold());
    println!("        {}", "lMMMMM;     dMMMMc".yellow().bold());
    println!("           {}", "lMMMMMMMMMM:".bright_yellow().bold());
    println!("               {}", "lMM;".yellow().bold());
    println!("{}", "    ═══════════════════════════════".yellow().bold());

    println!("\n{}\n", "=== OPTION 5: Rainbow Gradient ===".bold());
    println!("{}", "     .MMMMMMMMMM,".red().bold());
    println!("  {}", ",MMMMO     cMMMMMc".bright_red().bold());
    println!(" {}", "WMMMMx       .MMMMMM".yellow().bold());
    println!("{}", "0MMMMM'        ;MMMMM0".bright_yellow().bold());
    println!("{}", ":MMMMM.           :MMM".green().bold());
    println!("      {}", "0MMMMM;        .MMMMMK".cyan().bold());
    println!("       {}", "MMMMMM.       dMMMMM".blue().bold());
    println!("        {}", "lMMMMM;     dMMMMc".bright_blue().bold());
    println!("           {}", "lMMMMMMMMMM:".magenta().bold());
    println!("               {}", "lMM;".bright_magenta().bold());

    println!("\n{}\n", "=== OPTION 6: Blue Fire ===".bold());
    println!("{}", "╭─────────────────────────────────╮".bright_blue().bold());
    println!("{}     {}                  {}", "│".bright_blue().bold(), ".MMMMMMMMMM,".white().bold(), "│".bright_blue().bold());
    println!("{}  {}              {}", "│".bright_blue().bold(), ",MMMMO     cMMMMMc".bright_white().bold(), "│".bright_blue().bold());
    println!("{} {}               {}", "│".bright_blue().bold(), "WMMMMx       .MMMMMM".bright_cyan().bold(), "│".bright_blue().bold());
    println!("{}{} {}", "│".bright_blue().bold(), "0MMMMM'        ;MMMMM0".cyan().bold(), "│".bright_blue().bold());
    println!("{}{}{}", "│".bright_blue().bold(), ":MMMMM.           :MMM".bright_blue().bold(), "│".bright_blue().bold());
    println!("{}      {} {}", "│".bright_blue().bold(), "0MMMMM;        .MMMMMK".blue().bold(), "│".bright_blue().bold());
    println!("{}       {}        {}", "│".bright_blue().bold(), "MMMMMM.       dMMMMM".bright_cyan().bold(), "│".bright_blue().bold());
    println!("{}        {}       {}", "│".bright_blue().bold(), "lMMMMM;     dMMMMc".cyan().bold(), "│".bright_blue().bold());
    println!("{}           {}  {}", "│".bright_blue().bold(), "lMMMMMMMMMM:".bright_blue().bold(), "│".bright_blue().bold());
    println!("{}               {}               {}", "│".bright_blue().bold(), "lMM;".blue().bold(), "│".bright_blue().bold());
    println!("{}", "╰─────────────────────────────────╯".bright_blue().bold());
}
