//! `/ae2debug` (operators, level 2), the Horizon port's debug command (me.rs `debug_command`): so far `eta`, each busy
//! crafting CPU of the source's dimension with its time left, why it stands and what it holds.

use mod_api::*;

/// `register_commands`.
pub(crate) fn register(e: &mut RegisterCommandsEvent<'_>) {
    e.commands.push(Command::op("ae2debug eta", 2).executes(|host, source, _| {
        // A player's or a command block's dimension (a player's state is swapped in); the console's, the overworld.
        let dim: &'static str = match source {
            CommandSource::Console { .. } => "minecraft:overworld",
            _ => host.dimension(),
        };
        for line in crate::autocraft::eta::debug_lines(host, dim) {
            let _ = host.commands().feedback(Text::literal(line));
        }
        Ok(1)
    }));
}
