package com.example.logfilterproblemdemo;

/**
 * A deterministic two-press confirmation gate for destructive demo actions.
 *
 * <p>The first press arms an action. A second press of the same action within the timeout
 * confirms it. Selecting a different action or waiting past the deadline starts a new arming
 * window.
 */
public final class ArmingGate {
    public enum Decision {
        ARMED,
        CONFIRMED
    }

    private final long timeoutMs;
    private String armedActionId;
    private long armedUntilMs;

    public ArmingGate(long timeoutMs) {
        if (timeoutMs <= 0) {
            throw new IllegalArgumentException("timeoutMs must be positive");
        }
        this.timeoutMs = timeoutMs;
    }

    public Decision accept(String actionId, long nowMs) {
        if (actionId == null || actionId.isEmpty()) {
            throw new IllegalArgumentException("actionId must not be empty");
        }

        if (actionId.equals(armedActionId) && nowMs <= armedUntilMs) {
            clear();
            return Decision.CONFIRMED;
        }

        armedActionId = actionId;
        armedUntilMs = nowMs + timeoutMs;
        return Decision.ARMED;
    }

    public void clear() {
        armedActionId = null;
        armedUntilMs = 0;
    }
}
