package de.feschber.peg_solitaire;

import com.google.androidgamesdk.GameActivity;

public class MainActivity extends GameActivity {
    static {
        System.loadLibrary("peg_solitaire");
    }
}
