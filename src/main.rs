fn main() {

    let project_name = "Postman CLI";
    let mut articles_processed = 0;

    let feeds = vec!["dronedj.com", "dji.com", "techcrunch.com"];

    println!("--- Запуск відладки проєкту {} ---", project_name);

    for single_feed in feeds {
        articles_processed += 1;
        
        println!("Парсимо джерело: {}, Оброблено статей: {}", single_feed, articles_processed);
    }

    println!("--- Роботу завершено. Разом оброблено: {} ---", articles_processed);
}
