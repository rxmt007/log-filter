package com.example.logfilterproblemdemo;

import static org.junit.Assert.assertEquals;

import org.junit.Test;

public final class DemoUiModeTest {
    @Test
    public void ordinaryTouchDeviceUsesPhoneScreen() {
        DemoUiMode mode = DemoUiMode.select(false, false);

        assertEquals(DemoUiMode.PHONE, mode);
    }

    @Test
    public void officialTelevisionUiModeUsesTvScreen() {
        DemoUiMode mode = DemoUiMode.select(true, false);

        assertEquals(DemoUiMode.TELEVISION, mode);
    }

    @Test
    public void leanbackDeviceUsesTvScreenEvenWhenUiModeIsMissing() {
        assertEquals(DemoUiMode.TELEVISION, DemoUiMode.select(false, true));
    }
}
