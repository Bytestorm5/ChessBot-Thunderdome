#![no_std]
#[macro_use]
extern crate alloc;
use alloc::{
    string::{String, ToString},
    vec::Vec,
};

use core::{convert::TryFrom, fmt::Debug, cmp::Ordering};

extern crate std;

mod board;
pub use board::{Board, BoardBuilder};

mod game;
pub use game::{Game, GameAction, GameError, GameOver};

mod square;
pub use square::{Square, EMPTY_SQUARE};

mod piece;
pub use piece::Piece;

mod position;
pub use position::*;

mod util;
pub use util::*;

use rayon::prelude::*;
use std::sync::{Arc, Mutex};
use dashmap::DashMap;

pub const WHITE: Color = Color::White;
pub const BLACK: Color = Color::Black;

/// The result of a move being played on the board.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum GameResult {
    /// The game is not finished, and the game is still in play.
    Continuing(Board),
    /// One player, the victor, checkmated the other.
    /// This stores the color of the winner.
    Victory(Color),
    /// The game is drawn. This can be a result of the current player
    /// having no legal moves and not being in check, or because
    /// both players have insufficient material on the board.
    ///
    /// Insufficient material consists of:
    /// 1. The player only has a king
    /// 2. The player only has a king and a knight
    /// 3. The player only has a king and two knights
    /// 4. The player only has a king and a bishop
    /// 5. The player only has a king and two bishops
    ///
    /// In a regular game of chess, threefold repetition also triggers
    /// a stalemate, but this engine does not have builtin support for
    /// threefold repetition detection yet.
    Stalemate,
    /// An illegal move was made. This can include many things,
    /// such as moving a piece through another piece, attempting
    /// to capture an allied piece, moving non-orthogonally or
    /// non-diagonally, or non-knight-like according the rules
    /// governing the movement of the piece. Additionally,
    /// moves that put the player in check, (for example, moving a pinned piece),
    /// are also illegal.
    IllegalMove(Move),
}

/// The color of a piece.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Color {
    White,
    Black,
}
impl Color {
    fn invert(&self) -> Color {
        match self {
            Color::White => Color::Black,
            Color::Black => Color::White,
        }
    }
}
impl core::fmt::Display for Color {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> Result<(), core::fmt::Error> {
        write!(
            f,
            "{}",
            match self {
                Self::White => "White",
                Self::Black => "Black",
            }
        )
    }
}

/// A color can be inverted using the `!` operator.
/// `!Color::White` becomes `Color::Black` and vice versa.
impl core::ops::Not for Color {
    type Output = Self;
    fn not(self) -> Self {
        match self {
            Self::White => Self::Black,
            Self::Black => Self::White,
        }
    }
}

/// A move that can be applied to a board.
/// When applied to a board, the board assumes that the move is
/// being applied for the current turn's player.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Move {
    /// If the current player is white, move the king to the C1 square, and the kingside rook to
    /// the D1 square. If the current player is black, however, move the king to the C8 square,
    /// and the kingside rook to the D8 square.
    ///
    /// Castling can only be performed if
    /// 1. The king has not moved at all since the game began
    /// 2. The respective rook (kingside or queenside) has also not moved
    /// 3. The square adjacent to the king on the respective side is not threatened by an enemy piece
    ///
    /// If all of these conditions are satisfied, castling is a legal move
    QueenSideCastle,
    /// If the current player is white, move the king to the G1 square, and the kingside rook to
    /// the F1 square. If the current player is black, however, move the king to the G8 square,
    /// and the kingside rook to the F8 square.
    KingSideCastle,
    /// Move a piece from one square to another, with optional promotion.
    /// This can allow the player to capture another piece, by
    /// simply moving a piece to the position of an enemy piece.
    ///
    /// Additionally, this can be used to [en-passant capture](https://en.wikipedia.org/wiki/En_passant),
    /// even though the en-passant square itself does not contain any capturable pieces.
    ///
    /// En-passant captures MUST be performed with a pawn, upon an enemy pawn
    /// that has just surpassed it by move two squares. An en-passant capture
    /// must also be performed the turn immediately after the enemy pawn surpasses
    /// the allied pawn. After the one turn a player has to en-passant capture, the
    /// en-passant square is forgotten and can no longer be used.
    Piece(Position, Position),
    Promotion(Position, Position, Piece),
    /// When played by another player, it awards victory to the other.
    Resign,
}

/// Try to parse a Move from a string.
///
/// Possible valid formats include:
/// - `"resign"`
/// - `"resigns"`
/// - `"castle queenside"`
/// - `"O-O-O"` (correct notation)
/// - `"o-o-o"` (incorrect notation, but will accept)
/// - `"0-0-0"` (incorrect notation, but will accept)
/// - `"castle kingside"`
/// - `"O-O"` (correct notation)
/// - `"o-o"` (incorrect notation, but will accept)
/// - `"0-0"` (incorrect notation, but will accept)
/// - `"e2e4"`
/// - `"e2 e4"`
/// - `"e2 to e4"`
///
/// Parsing a move such as `"knight to e4"` or `"Qxe4"` will NOT work.
impl TryFrom<String> for Move {
    type Error = String;

    fn try_from(repr: String) -> Result<Self, Self::Error> {
        let repr = repr.trim().to_string();

        Ok(match repr.as_str() {
            "resign" | "resigns" => Self::Resign,
            "queenside castle" | "castle queenside" | "O-O-O" | "0-0-0" | "o-o-o" => {
                Self::QueenSideCastle
            }
            "kingside castle" | "castle kingside" | "O-O" | "0-0" | "o-o" => Self::KingSideCastle,
            other => {
                let words = other.split_whitespace().collect::<Vec<&str>>();

                if words.len() == 1 && words[0].len() == 4 {
                    Self::Piece(
                        Position::pgn(&words[0][..2])?,
                        Position::pgn(&words[0][2..4])?,
                    )
                } else if words.len() == 2 {
                    Self::Piece(Position::pgn(words[0])?, Position::pgn(words[1])?)
                } else if words.len() == 3 && words[1] == "to" {
                    Self::Piece(Position::pgn(words[0])?, Position::pgn(words[2])?)
                } else if words.len() == 4 && words[1] == "to" {
                    let piece = Piece::try_from(words[3])?;
                    if piece.is_king() || piece.is_pawn() {
                        return Err(String::from("invalid promotion"));
                    }
                    Self::Promotion(Position::pgn(words[0])?, Position::pgn(words[2])?, piece)
                } else {
                    return Err(format!("invalid move format `{}`", other));
                }
            }
        })
    }
}

impl Move {
    /// Try to parse a Move from a string.
    ///
    /// Possible valid formats include:
    /// - `"resign"`
    /// - `"resigns"`
    /// - `"castle queenside"`
    /// - `"O-O-O"` (correct notation)
    /// - `"o-o-o"` (incorrect notation, but will accept)
    /// - `"0-0-0"` (incorrect notation, but will accept)
    /// - `"castle kingside"`
    /// - `"O-O"` (correct notation)
    /// - `"o-o"` (incorrect notation, but will accept)
    /// - `"0-0"` (incorrect notation, but will accept)
    /// - `"e2e4"`
    /// - `"e2 e4"`
    /// - `"e2 to e4"`
    ///
    /// Parsing a move such as `"knight to e4"` or `"Qxe4"` will NOT work.
    pub fn parse(repr: String) -> Result<Self, String> {
        Self::try_from(repr)
    }
}

impl core::fmt::Display for Move {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> Result<(), core::fmt::Error> {
        match self {
            // Move::EnPassant(from) => write!(f, "ep {}", from),
            Move::Piece(from, to) => write!(f, "{} to {}", from, to),
            Move::Promotion(from, to, piece) => {
                write!(f, "{} to {} {}", from, to, piece.get_name())
            }
            Move::KingSideCastle => write!(f, "O-O"),
            Move::QueenSideCastle => write!(f, "O-O-O"),
            Move::Resign => write!(f, "Resign"),
        }
    }
}

/// Evaluate a board and extract information, such as the best and worst moves.
pub trait Evaluate: Sized where Self: Sync {
    /// Get the value of the board using piece tables.
    fn value_for(&self, color: Color) -> f64;

    /// Get the value of the board based on the mobility of pieces
    fn mobility_value_for(&self, color: Color) -> f64;

    /// Get the value of the board based on the simple amount of pieces
    fn naive_value_for(&self, color: Color) -> f64;

    /// Get the value of the board based on how many squares each side controls
    fn control_value_for(&self, color: Color) -> f64;

    /// Get the value of the board based on the minimum distance of an enemy from the king
    fn closest_value_for(&self, color: Color) -> f64;

    /// Get the value of the board, minimizing the amount of material on the board
    fn trade_value_for(&self, color: Color) -> f64;

    /// Get the current player's color.
    fn get_current_player_color(&self) -> Color;

    /// Get the legal moves for the current player.
    fn get_legal_moves(&self) -> Vec<Move>;
    
    ///Get the legal moves for a given player.
    fn get_legal_moves_for(&self, color: Color) -> Vec<Move>;

    /// Apply a move to the board for evaluation.
    fn apply_eval_move(&self, m: Move) -> Self;

    //Create a concise string representation of the board for caching
    fn cache_repr(&self) -> String;

    /// Get the best move for the current player with `depth` number of moves
    /// of lookahead.
    ///
    /// This method returns
    /// 1. The best move
    /// 2. The number of boards evaluated to come to a conclusion
    /// 3. The rating of the best move
    ///
    /// It's best not to use the rating value by itself for anything, as it
    /// is relative to the other player's move ratings as well.
    fn get_best_next_move(&self, depth: i32, engine: Option<[f64; 6]>) -> (Move, u64, f64) {
        let legal_moves = self.get_legal_moves();        

        let color = self.get_current_player_color();

        let board_count = Arc::new(Mutex::new(0));
        let board_cache: Arc<Mutex<DashMap<String, f64>>> = Arc::new(Mutex::new(DashMap::new()));

        let arc_engine = Arc::new(engine);
        
        let (best_move, best_move_value) = legal_moves
        .par_iter()        
        .map(|mov| {
            let e = Arc::clone(&arc_engine);
            let c = Arc::clone(&board_cache);
            let b = self.clone();
            let bc = Arc::clone(&board_count);
            let value = b.apply_eval_move(*mov).minimax(
                depth,
                -1000000.0,
                1000000.0,
                false,
                color,
                &mut bc.lock().unwrap(),
                *e,
                &mut c.lock().unwrap(),
            );
            //println!("Move {}: {}", mov.to_string(), value.to_string());
            (mov, value)
        })
        .max_by(|(_, a), (_, b)| a.partial_cmp(&b).unwrap_or(Ordering::Equal))
        .unwrap();
        let count: u64 = *board_count.lock().unwrap();
        (*best_move, count, best_move_value)
    }

    /// Get the best move for the current player with `depth` number of moves
    /// of lookahead.
    ///
    /// This method returns
    /// 1. The best move
    /// 2. The number of boards evaluated to come to a conclusion
    /// 3. The rating of the best move
    ///
    /// It's best not to use the rating value by itself for anything, as it
    /// is relative to the other player's move ratings as well.
    fn get_worst_next_move(&self, depth: i32, engine: Option<[f64; 6]>) -> (Move, u64, f64) {
        let legal_moves = self.get_legal_moves();        

        let color = self.get_current_player_color();

        let board_count = Arc::new(Mutex::new(0));
        let board_cache: Arc<Mutex<DashMap<String, f64>>> = Arc::new(Mutex::new(DashMap::new()));

        let arc_engine = Arc::new(engine);
        
        let (best_move, best_move_value) = legal_moves
        .par_iter()        
        .map(|mov| {
            let e = Arc::clone(&arc_engine);
            let c = Arc::clone(&board_cache);
            let b = self.clone();
            let bc = Arc::clone(&board_count);
            let value = b.apply_eval_move(*mov).minimax(
                depth,
                -1000000.0,
                1000000.0,
                false,
                color,
                &mut bc.lock().unwrap(),
                *e,
                &mut c.lock().unwrap(),
            );
            (mov, value)
        })
        .max_by(|(_, a), (_, b)| a.partial_cmp(&b).unwrap_or(Ordering::Equal))
        .unwrap();
        let count: u64 = *board_count.lock().unwrap();
        (*best_move, count, best_move_value)
    }

    /// Perform minimax on a certain position, and get the minimum or maximum value
    /// for a board. To get the best move, you minimize the values of the possible outcomes from your
    /// own position, and maximize the values of the replies made by the other player.
    ///
    /// In other words, choose moves with the assumption that your opponent will make the
    /// best possible replies to your moves. Moves that are seemingly good, but are easily countered,
    /// are categorically eliminated by this algorithm.
    fn minimax(
        &self,
        depth: i32,
        mut alpha: f64,
        mut beta: f64,
        is_maximizing: bool,
        getting_move_for: Color,
        board_count: &mut u64,
        engine: Option<[f64; 6]>,
        mut cache: &mut DashMap<String, f64>,
    ) -> f64 {        
        let eval_engine = match engine {
            Some(a) => a,
            None => [1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        };
        if depth == 0 {
            *board_count += 1;
            let mut eval = 0.0;

            if eval_engine[0] != 0.0 {
                eval += self.value_for(getting_move_for) * eval_engine[0]
            }
            if eval_engine[1] != 0.0 {
                eval += self.mobility_value_for(getting_move_for) * eval_engine[1]
            }
            if eval_engine[2] != 0.0 {
                eval += self.naive_value_for(getting_move_for) * eval_engine[2]
            }
            if eval_engine[3] != 0.0 {
                eval += self.control_value_for(getting_move_for) * eval_engine[3]
            }
            if eval_engine[4] != 0.0 {
                eval += self.closest_value_for(getting_move_for) * eval_engine[4]
            }
            if eval_engine[5] != 0.0 {
                eval += self.trade_value_for(getting_move_for) * eval_engine[5]
            }
            cache.insert(self.cache_repr(), eval);
            return eval
        }

        let legal_moves = self.get_legal_moves();
        let mut best_move_value;

        if is_maximizing {
            best_move_value = -999999.0;

            for m in &legal_moves {
                let child_board_value;
                let repr = self.cache_repr();
                if cache.contains_key(&repr) {
                    child_board_value = *cache.get(&repr).unwrap();
                }
                else {
                    child_board_value = self.apply_eval_move(*m).minimax(
                        depth - 1,
                        alpha,
                        beta,
                        !is_maximizing,
                        getting_move_for,
                        board_count,
                        Some(eval_engine),
                        &mut cache
                    );
                }
                if child_board_value > best_move_value {
                    best_move_value = child_board_value;
                }

                if best_move_value > alpha {
                    alpha = best_move_value
                }

                if beta <= alpha {
                    return best_move_value;
                }
            }
        } else {
            best_move_value = 999999.0;

            for m in &legal_moves {
                let child_board_value;
                let repr = self.cache_repr();
                if cache.contains_key(&repr) {
                    child_board_value = *cache.get(&repr).unwrap()
                }
                else {
                    child_board_value = self.apply_eval_move(*m).minimax(
                        depth - 1,
                        alpha,
                        beta,
                        !is_maximizing,
                        getting_move_for,
                        board_count,
                        Some(eval_engine),
                        &mut cache
                    );
                }
                if child_board_value < best_move_value {
                    best_move_value = child_board_value;
                }

                if best_move_value < beta {
                    beta = best_move_value
                }

                if beta <= alpha {
                    return best_move_value;
                }
            }
        }

        best_move_value
    }
}
