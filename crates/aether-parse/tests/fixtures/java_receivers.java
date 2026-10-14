package com.acme.store;

import java.util.Map;
import com.acme.io.Session;
import com.acme.model.Order.Status;
import com.acme.model.Record;

class Ledger {
    private Pointer cursor;
    private Pointer deposit;
    private Line line;
    private Store cache;
    private final Account account;

    Ledger(Account account) {
        this.account = account;
    }

    Entry post(Account account, Event event) {
        for (Cursor cursor = account.cursor(); cursor.hasNext(); cursor.advance()) {
            cursor.mark();
        }
        if (event instanceof Deposit deposit && deposit.isLarge()) {
            deposit.review();
        }
        try (Session session = open()) {
            session.flush();
        } catch (LedgerException failure) {
            failure.report();
        }
        account.lines().forEach(line -> line.settle());
        return new Entry(nextNumber(), this.label());
    }

    void index(Map.Entry<String, Entry> pair) {
        pair.getValue();
    }

    static Builder builder() {
        return new Builder();
    }

    void configure() {
        Ledger.Builder builder = Ledger.builder();
        builder.limit(5);
        Runnable task = new Runnable() {
            private final Cache cache = new Cache();

            public void run() {
                cache.clear();
                reset();
            }
        };
    }

    void route(Object event) {
        switch (event) {
            case Withdrawal withdrawal -> withdrawal.cancel();
            default -> {}
        }
    }

    boolean done(Status status) {
        return status.isFinal();
    }

    void save(Record record) {}

    @Deprecated
    void reset() {}

    static void validate(int value) {}

    static class Builder {
        Builder limit(int value) {
            validate(value);
            return this;
        }
    }

    Session open() {
        return null;
    }

    int nextNumber() {
        return 1;
    }

    String label() {
        return "";
    }
}

record Entry(int number, String label) {
    Entry {
        check(number);
    }

    String describe() {
        return label.trim();
    }

    static void check(int number) {}
}

enum Currency {
    EUR,
    USD;

    private final Formatter formatter = new Formatter();

    String symbol() {
        return formatter.format(name());
    }
}

interface Journal extends Source {
    default void append(Entry entry) {
        entry.describe();
    }
}

class Factory {
    Builder start() {
        return new Builder();
    }

    <T> void keep(T value) {}
}

class Overloads {
    void find(String key) {}

    void find(String key, int limit) {}
}
