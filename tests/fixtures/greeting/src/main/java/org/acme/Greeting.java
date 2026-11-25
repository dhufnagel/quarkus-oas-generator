package org.acme;

import jakarta.validation.constraints.NotBlank;
import jakarta.validation.constraints.Size;
import java.time.LocalDate;
import java.util.List;
import java.util.Optional;

/** A greeting for one person. */
public record Greeting(
        @NotBlank @Size(max = 80) String name,
        Language language,
        Optional<LocalDate> birthday,
        List<String> tags) {

    public enum Language {
        GERMAN,
        ENGLISH
    }
}
