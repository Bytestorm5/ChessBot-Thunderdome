extern crate chess_engine;
use chess_engine::*;
use std::{
    io::{stdin, stdout, Write}, 
    thread,
    time,
};
use mongodb::{bson::{doc, Document, Bson}, Client, options::{ClientOptions, ServerApiVersion, ServerApi, UpdateOptions}};
use futures::stream::{TryStreamExt};
use dotenv::dotenv;
use std::env;
use std::time::{Duration, Instant};

fn get_cpu_move(b: &Board, w_engine: Option<[f64; 6]>, b_engine: Option<[f64; 6]>) -> Move {
    let mut depth = 4;
    let min_time = 10; //seconds

    let mut start = Instant::now();
    let (mut m, mut count, _) = if b.get_turn_color() == Color::White {
        b.get_best_next_move(depth, w_engine)
    } else {
        b.get_best_next_move(depth, b_engine)
    };
    while start.elapsed().as_secs() < min_time && count < 10000 {
        start = Instant::now();
        depth += 1;
        (m, count, _) = if b.get_turn_color() == Color::White {
            b.get_best_next_move(depth, w_engine)
        } else {
            b.get_best_next_move(depth, b_engine)
        };
    }
    let nodes_per_sec = ((count as f64) / (start.elapsed().as_secs_f64())).round();
    print!("CPU evaluated {} moves before choosing to ", count);
    match m {
        Move::Piece(from, to) | Move::Promotion(from, to, _) => {
            match (b.get_piece(from), b.get_piece(to)) {
                (Some(piece), Some(takes)) => print!(
                    "take {}({}) with {}({})",
                    takes.get_name(),
                    to,
                    piece.get_name(),
                    from
                ),
                (Some(piece), None) => {
                    print!("move {}({}) to {}", piece.get_name(), from, to)
                }
                _ => print!("move {} to {}", from, to),
            }
        }
        Move::KingSideCastle => {
            print!("castle kingside")
        }
        Move::QueenSideCastle => {
            print!("castle queenside")
        }
        Move::Resign => print!("resign"),
    }
    println!(" ({nodes_per_sec} nodes/sec at depth {depth})");

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
    
fn calculate_elo(player1_elo: f64, player2_elo: f64, result: GameResult) -> (f64, f64) {
    // Constants for the ELO system
    const K_FACTOR: f64 = 32.0;
    const ELO_DIFFERENCE_LIMIT: f64 = 400.0;
    const WIN_PROBABILITY_CONSTANT: f64 = 10.0;

    // Calculate the expected win probability for player 1
    let elo_difference = (player2_elo - player1_elo) / ELO_DIFFERENCE_LIMIT;
    let expected_win_probability = 1.0 / (1.0 + WIN_PROBABILITY_CONSTANT.powf(elo_difference));

    // Calculate the actual result
    let actual_result = match result {
        GameResult::Stalemate => 0.5,
        GameResult::Victory(_) => 1.0,
        GameResult::Continuing(_) => {return (player1_elo, player2_elo)},
        GameResult::IllegalMove(_) => {return (player1_elo, player2_elo)},
    };

    // Calculate the new ELO ratings for both players
    let player1_new_elo = player1_elo + K_FACTOR * (actual_result - expected_win_probability);
    let player2_new_elo = player2_elo + K_FACTOR * (expected_win_probability - actual_result);

    (player1_new_elo, player2_new_elo)
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
                println!("White Engine: {:?}", w_engine);
                white_set = true;
            }
            else {
                b_engine = engine_array(Some(eng).unwrap().get_str("engine").ok().unwrap());
                println!("Black Engine: {:?}", b_engine);
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
                    println!("{}: {} {} {} {} {} {}", 
                        b.get_turn_color(),
                        b.value_for(b.get_turn_color()),
                        b.mobility_value_for(b.get_turn_color()),
                        b.naive_value_for(b.get_turn_color()),
                        b.control_value_for(b.get_turn_color()),
                        b.closest_value_for(b.get_turn_color()),
                        b.trade_value_for(b.get_turn_color())
                    );

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
                        "status": format!("{} loses. {} is victorious.", !winner, winner),
                        "board": b.fen(),
                    };

                    let filter = doc! {"_id": max_id};
                    let options = UpdateOptions::builder().upsert(true).build();
                    let mut update = doc! {"$set": Bson::from(&game_state)};
                    game_col.update_one(filter,update,options).await.ok().unwrap();

                    let black_engine_bson = engine_col.find_one(doc! {"engine":engine_str(b_engine)}, None).await.ok().unwrap().unwrap();
                    let white_engine_bson = engine_col.find_one(doc! {"engine":engine_str(w_engine)}, None).await.ok().unwrap().unwrap();

                    let elos: (f64, f64) = if winner == Color::White {
                        calculate_elo(white_engine_bson.get_f64("elo").unwrap() as f64, black_engine_bson.get_f64("elo").unwrap() as f64, GameResult::Victory(winner))
                    } else {
                        let t = calculate_elo(black_engine_bson.get_f64("elo").unwrap(), white_engine_bson.get_f64("elo").unwrap(), GameResult::Victory(winner));
                        (t.1, t.0)
                    };                    

                    update = doc! {
                        "$set": Bson::from(doc! {
                            "elo":elos.0,
                        }),
                        "$inc": Bson::from(doc! {
                            "wins": if winner == Color::White { 1 } else { 0 },
                            "losses": if winner == Color::White { 0 } else { 1 }  
                        })
                    };                    
                    engine_col.update_one(doc! {"engine":engine_str(w_engine)}, update, None).await.ok().unwrap();

                    update = doc! {
                        "$set": Bson::from(doc! {
                            "elo":elos.1,
                        }),
                        "$inc": Bson::from(doc! {
                            "wins": if winner == Color::Black { 1 } else { 0 },
                            "losses": if winner == Color::Black { 0 } else { 1 }  
                        })
                    };                    
                    engine_col.update_one(doc! {"engine":engine_str(b_engine)}, update, None).await.ok().unwrap();

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
                    let mut update = doc! {"$set": Bson::from(&game_state)};
                    game_col.update_one(filter,update,options).await.ok().unwrap();

                    let black_engine_bson = engine_col.find_one(doc! {"engine":engine_str(b_engine)}, None).await.ok().unwrap().unwrap();
                    let white_engine_bson = engine_col.find_one(doc! {"engine":engine_str(w_engine)}, None).await.ok().unwrap().unwrap();

                    let elos: (f64, f64) = calculate_elo(white_engine_bson.get_f64("elo").unwrap(), black_engine_bson.get_f64("elo").unwrap(), GameResult::Stalemate);
                    update = doc! {
                        "$set": Bson::from(doc! {
                            "elo":elos.0,
                        }),
                        "$inc": Bson::from(doc! {
                            "draws": 1 
                        })
                    };                    
                    engine_col.update_one(doc! {"engine":engine_str(w_engine)}, update, None).await.ok().unwrap();

                    update = doc! {
                        "$set": Bson::from(doc! {
                            "elo":elos.1,
                        }),
                        "$inc": Bson::from(doc! {
                            "draws": 1 
                        })
                    };                    
                    engine_col.update_one(doc! {"engine":engine_str(b_engine)}, update, None).await.ok().unwrap();

                    break;
                }
            }
            thread::sleep(time::Duration::from_millis(1500))
        }
    }
    Ok(())
}
