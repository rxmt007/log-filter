package com.example.logfilterproblemdemo;

/**
 * Selects one of the demo's two independent screens.
 *
 * <p>The boolean inputs are kept outside Android framework types so this single routing rule can be
 * unit-tested without an emulator.
 */
enum DemoUiMode {
    PHONE,
    TELEVISION;

    static DemoUiMode select(boolean televisionUiMode, boolean hasLeanbackFeature) {
        return televisionUiMode || hasLeanbackFeature ? TELEVISION : PHONE;
    }
}
