package com.example.logfilterproblemdemo;

import static org.junit.Assert.assertEquals;

import org.junit.Test;

public final class ArmingGateTest {
    @Test
    public void sameActionWithinWindowIsConfirmed() {
        ArmingGate gate = new ArmingGate(5_000);

        assertEquals(ArmingGate.Decision.ARMED, gate.accept("java-crash", 1_000));
        assertEquals(ArmingGate.Decision.CONFIRMED, gate.accept("java-crash", 5_999));
    }

    @Test
    public void expiredActionMustBeArmedAgain() {
        ArmingGate gate = new ArmingGate(5_000);

        assertEquals(ArmingGate.Decision.ARMED, gate.accept("anr", 1_000));
        assertEquals(ArmingGate.Decision.ARMED, gate.accept("anr", 6_001));
    }

    @Test
    public void changingActionArmsTheNewAction() {
        ArmingGate gate = new ArmingGate(5_000);

        assertEquals(ArmingGate.Decision.ARMED, gate.accept("java-crash", 1_000));
        assertEquals(ArmingGate.Decision.ARMED, gate.accept("native-crash", 2_000));
        assertEquals(ArmingGate.Decision.CONFIRMED, gate.accept("native-crash", 2_100));
    }

    @Test
    public void clearDiscardsAnArmedAction() {
        ArmingGate gate = new ArmingGate(5_000);

        assertEquals(ArmingGate.Decision.ARMED, gate.accept("process-exit", 1_000));
        gate.clear();
        assertEquals(ArmingGate.Decision.ARMED, gate.accept("process-exit", 1_100));
    }
}
