//! A terminal's present state as the bytes that bring a terminal emulator of the same size, just
//! reset, to that state. The app keeps a grid per block where an ordinary terminal has one
//! screen, so the blocks are written out as the lines such a terminal would have scrolled past,
//! and what the shell or the running program draws on now comes last.

use std::ops::Range;

use crate::terminal::ShellLaunchState;
use crate::terminal::model::TerminalModel;
use crate::terminal::model::block::{Block, BlockState};
use crate::terminal::model::grid::Dimensions as _;
use crate::terminal::model::grid::grid_handler::Replay;
use crate::terminal::model::index::Point;
use crate::terminal::shell::ShellType;

/// Rows of earlier output a snapshot starts with. They are read while the terminal is locked,
/// which is why they are far fewer than a client is able to keep.
const HISTORY_ROWS: usize = 2000;
const REDUCED_HISTORY_ROWS: usize = 200;

/// A snapshot travels as base64 inside one frame, and has to stay well under the frame limit.
const MAX_SNAPSHOT_BYTES: usize = 3 * 1024 * 1024;

pub(super) struct TerminalSnapshot {
    pub cols: usize,
    pub rows: usize,
    pub bytes: Vec<u8>,
    /// Whether the shell's line editor takes the command line to start in the first column
    /// although it has printed a prompt, which holds for the output that follows as well.
    pub prompt_has_no_width: bool,
}

pub(super) fn terminal_snapshot(model: &TerminalModel) -> TerminalSnapshot {
    let size = model.block_list().size();
    let bytes = [HISTORY_ROWS, REDUCED_HISTORY_ROWS, 0]
        .into_iter()
        .map(|history_rows| replay(model, history_rows))
        .find(|bytes| bytes.len() <= MAX_SNAPSHOT_BYTES)
        .unwrap_or_default();
    TerminalSnapshot {
        cols: size.columns(),
        rows: size.rows(),
        bytes,
        prompt_has_no_width: matches!(
            prompt_printing(model, model.block_list().active_block()),
            PromptPrinting::WithoutWidth
        ),
    }
}

/// How a block's prompt reached the terminal.
enum PromptPrinting {
    /// As part of the command line, in the same grid.
    WithCommand,
    /// Marked as taking no room. The line editor then draws the command line from the first
    /// column, over the prompt, so the prompt is kept on a line of its own.
    WithoutWidth,
    /// Not at all: the shell only reported what its prompt would have been.
    Never,
}

fn prompt_printing(model: &TerminalModel, block: &Block) -> PromptPrinting {
    if block.honor_ps1() {
        return PromptPrinting::WithCommand;
    }
    match model.shell_launch_state() {
        ShellLaunchState::ShellSpawned {
            shell_type: ShellType::Zsh,
            ..
        } => PromptPrinting::WithoutWidth,
        ShellLaunchState::ShellSpawned {
            shell_type: ShellType::Bash | ShellType::Fish | ShellType::PowerShell,
            ..
        }
        | ShellLaunchState::DeterminingShell { .. } => PromptPrinting::Never,
    }
}

/// What the active block is at the moment, which decides how it is written.
enum Live {
    /// The shell's line editor draws the prompt and the command line.
    Prompt,
    /// A program draws on the block's output grid as on a screen of its own.
    Program,
    /// A program draws on the alternate screen.
    AlternateScreen,
    /// Nothing draws on the block; it is history like the blocks before it.
    Finished,
}

fn replay(model: &TerminalModel, history_rows: usize) -> Vec<u8> {
    let block_list = model.block_list();
    let mut replay = Replay::new();
    // Until the shell is set up, what it has printed is the setup script, which no block shows.
    if !block_list.is_bootstrapped() {
        return replay.into_bytes();
    }

    let active = block_list.active_block();
    let earlier = &block_list.blocks()[..block_list.active_block_index().0];
    let live = if model.is_alt_screen_active() {
        Live::AlternateScreen
    } else {
        match active.state() {
            BlockState::BeforeExecution => Live::Prompt,
            BlockState::Executing => Live::Program,
            BlockState::DoneWithExecution
            | BlockState::DoneWithNoExecution
            | BlockState::Background
            | BlockState::Static => Live::Finished,
        }
    };
    // Output that arrives while the shell waits at its prompt, which what is typed there is
    // until the shell runs it, is kept in a block of its own right before the active one. On
    // an ordinary terminal it follows the prompt.
    let (earlier, after_prompt) = match (&live, earlier.split_last()) {
        (Live::Prompt, Some((last, rest))) if last.is_background() && !last.finished() => {
            (rest, Some(last))
        }
        (Live::Prompt | Live::Program | Live::AlternateScreen | Live::Finished, _) => {
            (earlier, None)
        }
    };

    // Every mode a program sets is also set on the alternate screen's grid, whichever grid is
    // active, so that grid has the modes of the terminal as a whole. A block's grid starts
    // without those set before it.
    let modes = model.alt_screen().grid_handler().term_mode();
    let output = active.output_grid();
    let mut budget = history_rows;
    let active_history = match live {
        Live::Prompt => 0..0,
        Live::Program => last_rows(output.grid_handler().history_size(), &mut budget),
        Live::AlternateScreen | Live::Finished => last_rows(output.len(), &mut budget),
    };

    let scope = block_list.transcript_scope();
    let mut history = Vec::new();
    for block in earlier.iter().rev() {
        if budget == 0 {
            break;
        }
        if !block.is_visible(scope) {
            continue;
        }
        let output_rows = if block.should_hide_output_grid() {
            0
        } else {
            block.output_grid().len()
        };
        history.push((block, last_rows(output_rows, &mut budget)));
        budget = budget.saturating_sub(1);
    }
    for (block, output_rows) in history.into_iter().rev() {
        if !block.should_hide_command_grid() {
            replay_header(model, block, &mut replay);
        }
        block
            .output_grid()
            .grid_handler()
            .replay_lines(output_rows, &mut replay);
    }

    match live {
        Live::Prompt => {
            replay_prompt(model, active, &mut replay);
            let header = active.prompt_and_command_grid();
            header
                .grid_handler()
                .replay_lines_around_cursor(header.len(), &mut replay);
            if let Some(block) = after_prompt {
                let output = block.output_grid();
                output
                    .grid_handler()
                    .replay_lines_around_cursor(output.len(), &mut replay);
            }
            header.grid_handler().replay_modes(modes, &mut replay);
        }
        Live::Program => {
            replay_header(model, active, &mut replay);
            let grid = output.grid_handler();
            grid.replay_lines(active_history, &mut replay);
            // A program addresses rows of its grid, so the grid's first visible row has to be the
            // emulator's first. As many line feeds as the screen has rows scroll everything
            // written so far off the top, wherever the cursor was.
            replay.control(&"\n".repeat(grid.visible_rows()));
            grid.replay_screen(&mut replay);
            grid.replay_modes(modes, &mut replay);
        }
        Live::AlternateScreen => {
            replay_header(model, active, &mut replay);
            output
                .grid_handler()
                .replay_lines(active_history, &mut replay);
            replay.control("\x1b[?1049h");
            let grid = model.alt_screen().grid_handler();
            grid.replay_screen(&mut replay);
            grid.replay_modes(modes, &mut replay);
        }
        Live::Finished => {
            replay_header(model, active, &mut replay);
            output
                .grid_handler()
                .replay_lines(active_history, &mut replay);
        }
    }
    replay.into_bytes()
}

/// The last rows of `total` that `budget` allows, which they are taken from.
fn last_rows(total: usize, budget: &mut usize) -> Range<usize> {
    let count = total.min(*budget);
    *budget -= count;
    total - count..total
}

/// Writes the prompt of a block whose prompt is not part of its command line, on a line of its
/// own.
fn replay_prompt(model: &TerminalModel, block: &Block, replay: &mut Replay) {
    match prompt_printing(model, block) {
        PromptPrinting::WithCommand | PromptPrinting::Never => return,
        PromptPrinting::WithoutWidth => {}
    }
    let prompt = block.prompt_grid();
    let Some(last_row) = prompt.len().checked_sub(1) else {
        return;
    };
    let grid = prompt.grid_handler();
    grid.replay_text(
        Point::new(0, 0),
        Point::new(last_row, grid.columns()),
        replay,
    );
    replay.line_break();
}

/// Writes a block's prompt and command as the lines they took.
fn replay_header(model: &TerminalModel, block: &Block, replay: &mut Replay) {
    replay_prompt(model, block, replay);
    let header = block.prompt_and_command_grid();
    header.grid_handler().replay_lines(0..header.len(), replay);
}

#[cfg(test)]
#[path = "terminal_snapshot_tests.rs"]
mod tests;
