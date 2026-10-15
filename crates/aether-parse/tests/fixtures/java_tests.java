package com.acme.store;

import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Nested;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.params.ParameterizedTest;

@DisplayName("Inventory lookups")
class InventoryTest {
    private final Inventory inventory = new Inventory(null);

    @Test
    void findsStoredItem() {
        inventory.find("a");
    }

    /**
     * Missing keys come back empty.
     *
     * @see Inventory#find
     */
    @Test
    void testFind_missingKey() {}

    @ParameterizedTest
    @DisplayName("rejects blank keys")
    void rejectsBlank(String key) {}

    @org.junit.Test
    public void legacyStyleWorks() {}

    void helperIsNotATest() {}

    @Test
    @DisplayName("parses \"null\"")
    void parsesNull() {}

    @Test
    @DisplayName("caf\u00e9 menu")
    void cafeMenu() {}

    @Nested
    class WhenEmpty {
        @Test
        void returnsNothing() {}
    }

    @Nested
    class WhenFull {
        @Test
        void returnsNothing() {}
    }
}
