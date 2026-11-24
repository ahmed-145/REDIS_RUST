use std::usize;

use crate::database::db::Database;

pub async fn command_parser(db: &Database, command: &str) -> Result<String, String> {
    let splitted_commad: Vec<&str> = command.split_whitespace().collect();

    match splitted_commad.as_slice() {
        ["SET", key, value, "EXP", ttl] => {
            let ttl = ttl.parse::<u64>().map_err(|_| "Invalid time to live value")?;
            db.set(key.to_string(), value.to_string(), Some(ttl)).await;
            Ok("+OK\r\n".to_string())
        }
        ["SET", key, value] => {
            db.set(key.to_string(), value.to_string(), None).await;
            Ok("+OK\r\n".to_string())
        }
        ["GET", key] => {
            if let Some(value) = db.get(key).await {
                Ok(format!("${}\r\n{}\r\n", value.len(), value))
            } else {
                Ok("$-1\r\n".to_string())
            }
        }
        ["DEL", key] => Ok(format!(":{}\r\n", u8::from(db.delete(key).await))),
        //LIST
        ["LPUSH", key, value] => {
            db.lpush(key.to_string(), value.to_string()).await;
            Ok("+OK\r\n".to_string())
        }
        ["RPUSH", key, value] => {
            db.rpush(key.to_string(), value.to_string()).await;
            Ok("+OK\r\n".to_string())
        }
        ["LPOP", key] => {
            if let Some(value) = db.lpop(key).await {
                Ok(format!("${}\r\n{}\r\n", value.len(), value))
            } else {
                Ok("+OK\r\n".to_string())
            }
        }
        ["RPOP", key] => {
            if let Some(value) = db.rpop(key).await {
                Ok(format!("${}\r\n{}\r\n", value.len(), value))
            } else {
                Ok("+OK\r\n".to_string())
            }
        }
        ["LRANGE", key, start, end] => {
            let start = start.parse::<usize>().map_err(|_| "Invalid start index")?;
            let end = end.parse::<usize>().map_err(|_| "Invalid start index")?;

            if end < start {
                return Err("End index must not be less than start index".to_string());
            }

            if let Some(list) = db.lrange(start, end, key).await {
                let mut response = format!("*{}\r\n", list.len());
                for item in list {
                    response.push_str(&format!("${}\r\n{}\r\n", item.len(), item))
                }
                Ok(response)
            } else {
                Ok("NOT FOUND\r\n".to_string())
            }
        }
        //SET
        ["SADD", key, value] => {
            let add = db.sadd(key.to_string(), value.to_string()).await;
            Ok(format!("${}\r\n", if add { 1 } else { 0 }))
        }

        ["SREM", key, value] => {
            let add = db.srem(key, value).await;
            Ok(format!("${}\r\n", if add { 1 } else { 0 }))
        }

        ["SMEMBERS", key] => {
            if let Some(list) = db.smembers(key).await {
                let mut response = format!("*{}\r\n", list.len());
                for item in list {
                    response.push_str(&format!("${}\r\n{}\r\n", item.len(), item))
                }
                Ok(response)
            } else {
                Ok("NOT FOUND\r\n".to_string())
            }
        }

        ["SISMEMBER", key, member] => {
            Ok(format!(":{}\r\n", u8::from(db.sismember(key, member).await)))
        }

        //Ordered SET
        ["ZADD", key, score, member] => {
            let score = score.parse::<f64>().map_err(|_| "Invalid score")?;
            let added = db.zadd(key.to_string(), score, member.to_string()).await;
            Ok(format!(":{}\r\n", if added { 1 } else { 0 }))
        }

        ["ZREM", key, member] => {
            let removed = db.zrem(&key.to_string(), member.to_string()).await;
            Ok(format!(":{}\r\n", if removed { 1 } else { 0 }))
        }
        ["ZRANGE", key, start, end] => {
            let start = start.parse::<usize>().map_err(|_| "Invalid start index")?;
            let end = end.parse::<usize>().map_err(|_| "Invalid end index")?;
            if end < start {
                return Err("End index must not be less than start index".to_string());
            }

            if let Some(members) = db.zrange(&key.to_string(), start, end).await {
                let mut response = format!("*{}\r\n", members.len());
                for member in members {
                    response.push_str(&format!("${}\r\n{}\r\n", member.len(), member));
                }
                Ok(response)
            } else {
                Ok("$-1\r\n".to_string())
            }
        }
        ["ZSCORE", key, member] => {
            if let Some(score) = db.zscore(key, member).await {
                let score_str = score.to_string();
                Ok(format!("${}\r\n{}\r\n", score_str.len(), score_str))
            } else {
                Ok("$-1\r\n".to_string())
            }
        }

        _ => Ok("-ERR\r\nUnknow commnd, please try again!!".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn del_returns_integer_for_hit_and_miss() {
        let db = Database::new();
        command_parser(&db, "SET key value").await.unwrap();
        assert_eq!(command_parser(&db, "DEL key").await.unwrap(), ":1\r\n");
        assert_eq!(command_parser(&db, "DEL key").await.unwrap(), ":0\r\n");
    }

    #[tokio::test]
    async fn sismember_returns_integer_for_hit_and_misses() {
        let db = Database::new();
        command_parser(&db, "SADD colors blue").await.unwrap();
        for (command, expected) in [
            ("SISMEMBER colors blue", ":1\r\n"),
            ("SISMEMBER colors red", ":0\r\n"),
            ("SISMEMBER missing blue", ":0\r\n"),
        ] {
            assert_eq!(command_parser(&db, command).await.unwrap(), expected);
        }
    }

    #[tokio::test]
    async fn lrange_parses_key_before_indices() {
        let db = Database::new();
        command_parser(&db, "RPUSH items first").await.unwrap();
        command_parser(&db, "RPUSH items second").await.unwrap();
        let result = command_parser(&db, "LRANGE items 1 1").await.unwrap();
        assert_eq!(result, "*1\r\n$6\r\nsecond\r\n");
    }

    #[tokio::test]
    async fn zadd_parses_key_then_score_then_member() {
        let db = Database::new();
        let reply = command_parser(&db, "ZADD leaderboard 2.5 alice").await.unwrap();
        assert_eq!(reply, ":1\r\n");
        assert_eq!(db.zscore("leaderboard", "alice").await, Some(2.5));
    }

    #[tokio::test]
    async fn invalid_exp_and_reversed_ranges_return_errors() {
        let db = Database::new();
        command_parser(&db, "RPUSH items first").await.unwrap();
        command_parser(&db, "ZADD leaderboard 1 alice").await.unwrap();
        for (command, expected) in [
            ("SET key value EXP nope", "Invalid time to live value"),
            ("LRANGE items 2 0", "End index must not be less than start index"),
            ("ZRANGE leaderboard 2 0", "End index must not be less than start index"),
        ] {
            assert_eq!(command_parser(&db, command).await.unwrap_err(), expected);
        }
        assert_eq!(db.get("key").await, None);
    }
}
