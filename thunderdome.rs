extern crate chess_engine;
use chess_engine::*;
use std::{
    convert::TryFrom,
    io::{stdin, stdout, Write}, future::Future,
};
use mongodb::{bson::{doc, Document, document, Bson}, Client, Collection, Cursor, options::{ClientOptions, ServerApiVersion, ServerApi, UpdateOptions}};
use futures::stream::{StreamExt, TryStreamExt};
use serde::{Deserialize, Serialize};
use dotenv::dotenv;
use std::env;

fn input(prompt: impl std::fmt::Display) -> String {
    let mut s = String::new();
    print!("{}", prompt);
    let _ = stdout().flush();
    stdin()
        .read_line(&mut s)
        .expect("Did not enter a correct string");
    s
}

fn get_cpu_move(b: &Board, w_engine: Option<[f64; 6]>, b_engine: Option<[f64; 6]>) -> Move {
    let depth = 3;
    let (m, count, _) = if b.get_turn_color() == Color::White {
        b.get_best_next_move(depth, w_engine)
    } else {
        b.get_best_next_move(depth, b_engine)
    };

    print!("CPU evaluated {} moves before choosing to ", count);
    match m {
        Move::Piece(from, to) | Move::Promotion(from, to, _) => {
            match (b.get_piece(from), b.get_piece(to)) {
                (Some(piece), Some(takes)) => println!(
                    "take {}({}) with {}({})",
                    takes.get_name(),
                    to,
                    piece.get_name(),
                    from
                ),
                (Some(piece), None) => {
                    println!("move {}({}) to {}", piece.get_name(), from, to)
                }
                _ => println!("move {} to {}", from, to),
            }
        }
        Move::KingSideCastle => {
            println!("castle kingside")
        }
        Move::QueenSideCastle => {
            println!("castle queenside")
        }
        Move::Resign => println!("resign"),
    }

    m
}

fn engine_array(engine_str: &str) -> [f64; 6] {
    let mut result = [0.0; 6];
    for (i, c) in engine_str.chars().enumerate() {
        let num = c.to_digit(10).unwrap() as f64;
        result[i] = num;
    }
    result
}
fn engine_str(engine_arr: [f64;6]) -> String {
    let mut result = "".to_string();
    for i in engine_arr {
        result = format!("{result}{i}");
    }
    result
}
    

#[tokio::main]
async fn main() -> Result<(), String> {    
    println!("THUNDERDOME!");
    dotenv().ok();

    let uri = env::var("MONGO_CONNECTION_STRING").unwrap();
    let mut client_options =tokio::task::spawn_blocking(move || {
        ClientOptions::parse(&uri).unwrap()
    }).await.unwrap();

    // Set the server_api field of the client_options object to Stable API version 1
    let server_api = ServerApi::builder().version(ServerApiVersion::V1).build();
    client_options.server_api = Some(server_api);
    // Create a new client and connect to the server
    let client = Client::with_options(client_options).ok().unwrap();
    // Send a ping to confirm a successful connection
    client
        .database("admin")
        .run_command(doc! {"ping": 1}, None)
        .await.ok();
    println!("Pinged your deployment. You successfully connected to MongoDB!");

    let engine_col = client.database("ChessThunderdome").collection::<Document>("engines");
    let game_col = client.database("ChessThunderdome").collection::<Document>("games");
    loop {

        let mut engine_selection = engine_col.aggregate([
            doc! {
                "$sample": doc! {
                    "size": 2
                }
            }
        ], None).await.ok().unwrap();
        
        let mut w_engine: [f64;6] = [0.0; 6];
        let mut b_engine: [f64;6] = [0.0; 6];

        let mut white_set = false;

        while let Some(eng) = engine_selection.try_next().await.ok().unwrap() {
            if !white_set {
                w_engine = engine_array(Some(eng).unwrap().get_str("engine").ok().unwrap());
                white_set = true;
            }
            else {
                b_engine = engine_array(Some(eng).unwrap().get_str("engine").ok().unwrap());
            }
        }

        let mut b = Board::default();

        let pipeline = vec![
                doc! {
                    "$group": {
                        "_id": null,
                        "maxId": { "$max": "$_id" }
                    }
                },
                doc! {
                    "$project": {
                        "_id": 0,
                        "maxId": 1
                    }
                }
            ];

        let mut cursor = game_col.aggregate(pipeline, None).await.ok().unwrap();
        let mut max_id = 0;
        while let Some(result) = cursor.try_next().await.ok().unwrap() {
            max_id = result.get_i32("maxId").unwrap();
        }
        max_id += 1;

        loop {
            let m = get_cpu_move(&b, Some(w_engine), Some(b_engine));          

            match b.play_move(m) {
                GameResult::Continuing(next_board) => {
                    b = next_board;
                    println!("{}", b.fen());

                    let next_move: String;
                    if b.get_turn_color() == Color::White {
                        next_move = "White to play".to_string();
                    }
                    else {
                        next_move = "Black to play".to_string();
                    }

                    let game_state = doc! {
                        "_id": max_id,
                        "black_engine": engine_str(b_engine),
                        "white_engine": engine_str(w_engine),
                        "status": next_move,
                        "board": b.fen(),
                    };

                    let filter = doc! {"_id": max_id};
                    let options = UpdateOptions::builder().upsert(true).build();
                    let update = doc! {"$set": Bson::from(&game_state)};
                    game_col.update_one(filter,update,options).await.ok().unwrap();
                }

                GameResult::Victory(winner) => {
                    println!("{}", b);
                    println!("{} loses. {} is victorious.", !winner, winner);

                    let game_state = doc! {
                        "_id": max_id,
                        "black_engine": engine_str(b_engine),
                        "white_engine": engine_str(w_engine),
                        "status": winner.to_string(),
                        "board": b.fen(),
                    };

                    let filter = doc! {"_id": max_id};
                    let options = UpdateOptions::builder().upsert(true).build();
                    let update = doc! {"$set": Bson::from(&game_state)};
                    game_col.update_one(filter,update,options).await.ok().unwrap();

                    break;
                }

                GameResult::IllegalMove(x) => {
                    eprintln!("{} is an illegal move.", x);
                }

                GameResult::Stalemate => {
                    println!("Drawn game.");

                    let game_state = doc! {
                        "_id": max_id,
                        "black_engine": engine_str(b_engine),
                        "white_engine": engine_str(w_engine),
                        "status": "Draw".to_string(),
                        "board": b.fen(),
                    };

                    let filter = doc! {"_id": max_id};
                    let options = UpdateOptions::builder().upsert(true).build();
                    let update = doc! {"$set": Bson::from(&game_state)};
                    game_col.update_one(filter,update,options).await.ok().unwrap();

                    break;
                }
            }
        }
    }
    Ok(())
}
