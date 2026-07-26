package com.example.logfilterproblemdemo;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.os.SystemClock;
import android.util.Log;

/**
 * Blocks a foreground broadcast long enough for ActivityManager's broadcast watchdog to report a
 * real ANR.
 */
public final class AnrBroadcastReceiver extends BroadcastReceiver {
    public static final String ACTION_TRIGGER_ANR =
            "com.example.logfilterproblemdemo.action.TRIGGER_ANR";

    private static final String TAG = "LogFilterDemo";

    @Override
    public void onReceive(Context context, Intent intent) {
        if (!ACTION_TRIGGER_ANR.equals(intent.getAction())) {
            return;
        }

        Log.i(TAG, "Beginning 30 second foreground-broadcast block for ANR demonstration");
        SystemClock.sleep(30_000);
        Log.i(TAG, "Finished foreground-broadcast block");
    }
}
