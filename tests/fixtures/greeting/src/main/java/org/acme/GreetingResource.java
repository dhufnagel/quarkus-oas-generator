package org.acme;

import jakarta.validation.Valid;
import jakarta.ws.rs.DELETE;
import jakarta.ws.rs.DefaultValue;
import jakarta.ws.rs.GET;
import jakarta.ws.rs.POST;
import jakarta.ws.rs.Path;
import jakarta.ws.rs.PathParam;
import jakarta.ws.rs.QueryParam;
import java.util.List;

@Path("/greetings")
public class GreetingResource {

    @GET
    public List<Greeting> listGreetings(@QueryParam("limit") @DefaultValue("10") int limit) {
        return List.of();
    }

    @GET
    @Path("/{name}")
    public Greeting getGreeting(@PathParam("name") String name) {
        return null;
    }

    @POST
    public Greeting createGreeting(@Valid Greeting greeting) {
        return greeting;
    }

    @DELETE
    @Path("/{name}")
    public void deleteGreeting(@PathParam("name") String name) {}
}
