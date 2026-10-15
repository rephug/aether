package com.acme.store;

import java.util.List;
import java.util.Optional;
import com.acme.db.Connection;
import org.slf4j.Logger;
import static com.acme.util.Checks.requireKey;

/** Stores items. */
public class Inventory extends BaseStore implements Repository<Item>, AutoCloseable {
    private final Connection connection;
    private final Cache cache = new Cache();

    public Inventory(Connection connection) {
        super(connection);
        this.connection = connection;
    }

    @Override
    public Optional<Item> find(String key) {
        requireKey(key);
        Item cached = cache.lookup(key);
        if (cached != null) {
            return Optional.of(cached);
        }
        return Optional.ofNullable(load(key));
    }

    private Item load(String key) {
        var row = new Row(key);
        row.validate();
        connection.fetch(row);
        this.connection.close();
        super.audit(key);
        return Item.from(row);
    }

    void trace(Logger logger, Item item) {
        logger.info(item.describe());
    }

    public List<Item> all(Filter filter) {
        return items().stream().filter(filter::matches).toList();
    }

    public abstract static class Visitor {
        abstract void visit(Item item);

        void visitAll(List<Item> items) {
            for (Item item : items) {
                visit(item);
                item.describe();
            }
        }
    }

    public void close() {
        Runnable task = new Runnable() {
            public void run() {
                connection.close();
            }
        };
        task.run();
    }
}

interface Repository<T> {
    Optional<T> find(String key);

    default boolean exists(String key) {
        return find(key).isPresent();
    }
}

enum Status implements Labeled {
    ACTIVE,
    RETIRED;

    String label() {
        return name().toLowerCase();
    }
}

record Item(String key, Price price) {
    static Item from(Row row) {
        return new Item(row.key(), Price.zero());
    }
}

@interface Audited {}
