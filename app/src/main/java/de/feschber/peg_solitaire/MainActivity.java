package de.feschber.peg_solitaire;

import android.app.NativeActivity;

public class MainActivity extends NativeActivity {
    static {
        System.loadLibrary("peg_solitaire");
    }
}
